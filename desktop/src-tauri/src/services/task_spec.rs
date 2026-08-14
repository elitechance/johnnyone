//! `task.yml` parse + path jail + DAG topo-sort.

use serde::Deserialize;
use std::path::{Component, Path};

/// Parsed task contract. Planner emission is initiative 2; this crate only consumes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub files: Vec<String>,
    pub verify: String,
    #[serde(default)]
    pub must_contain: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub ctx: Option<u32>,
    #[serde(default)]
    pub mock: Option<String>,
    #[serde(default)]
    pub new: Option<Vec<String>>,
    /// Optional jailed subdirectory used as the verify / kloo child cwd.
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Parse a YAML body and enforce the jail / non-empty rules.
pub fn parse_task_yaml(raw: &str) -> Result<TaskSpec, String> {
    let spec: TaskSpec = serde_yaml::from_str(raw)
        .map_err(|e| format!("shape: unparseable task.yml: {}", e))?;
    validate_spec(&spec)?;
    Ok(spec)
}

/// Load `<task_dir>/task.yml` and require `id` == the directory name.
pub fn load_task_spec(task_dir: &Path) -> Result<TaskSpec, String> {
    let path = task_dir.join("task.yml");
    let raw = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "shape: missing task.yml at {}: {}",
            path.display(),
            e
        )
    })?;
    let spec = parse_task_yaml(&raw)?;
    let dir_name = task_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if spec.id != dir_name {
        return Err(format!(
            "shape: task.yml id '{}' does not match directory name '{}'",
            spec.id, dir_name
        ));
    }
    Ok(spec)
}

fn validate_task_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("shape: empty id".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!("shape: invalid task id '{}'", id));
    }
    Ok(())
}

fn validate_spec(spec: &TaskSpec) -> Result<(), String> {
    validate_task_id(&spec.id)?;
    if spec.files.is_empty() {
        return Err("shape: empty files".to_string());
    }
    if spec.verify.trim().is_empty() {
        return Err("shape: empty verify".to_string());
    }
    for path in &spec.files {
        jail_rel_path(path)?;
    }
    if let Some(new_paths) = &spec.new {
        for path in new_paths {
            jail_rel_path(path)?;
            if !spec.files.iter().any(|f| f == path) {
                return Err(format!(
                    "shape: new path '{}' is not in files[]",
                    path
                ));
            }
        }
    }
    if let Some(mock) = &spec.mock {
        jail_rel_path(mock)?;
    }
    if let Some(cwd) = &spec.cwd {
        jail_rel_path(cwd)?;
    }
    Ok(())
}

pub(crate) fn jail_rel_path(path: &str) -> Result<(), String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("shape: empty path".to_string());
    }
    let p = Path::new(trimmed);
    if p.is_absolute() {
        return Err(format!("shape: path must be relative: {}", trimmed));
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("shape: path must not contain '..': {}", trimmed));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DagError {
    Cycle { nodes: Vec<String> },
    UnknownDep { from: String, to: String },
    SelfDep { id: String },
}

impl std::fmt::Display for DagError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DagError::Cycle { nodes } => write!(f, "shape: cycle in depends_on: {}", nodes.join(",")),
            DagError::UnknownDep { from, to } => {
                write!(f, "shape: {} depends_on unknown id {}", from, to)
            }
            DagError::SelfDep { id } => write!(f, "shape: {} depends_on itself", id),
        }
    }
}

/// Deterministic Kahn topo-sort. Ready-set is sorted by `id`.
pub fn topo_sort(tasks: &[TaskSpec]) -> Result<Vec<String>, DagError> {
    use std::collections::{HashMap, HashSet, VecDeque};

    let ids: HashSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut incoming: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in tasks {
        incoming.entry(t.id.as_str()).or_default();
        outgoing.entry(t.id.as_str()).or_default();
        for dep in &t.depends_on {
            if dep == &t.id {
                return Err(DagError::SelfDep { id: t.id.clone() });
            }
            if !ids.contains(dep.as_str()) {
                return Err(DagError::UnknownDep {
                    from: t.id.clone(),
                    to: dep.clone(),
                });
            }
            incoming.entry(t.id.as_str()).or_default().insert(dep.as_str());
            outgoing.entry(dep.as_str()).or_default().push(t.id.as_str());
        }
    }

    let mut ready: Vec<&str> = incoming
        .iter()
        .filter(|(_, deps)| deps.is_empty())
        .map(|(id, _)| *id)
        .collect();
    ready.sort_unstable();
    let mut queue: VecDeque<&str> = ready.into();
    let mut order = Vec::new();

    while let Some(id) = queue.pop_front() {
        order.push(id.to_string());
        if let Some(kids) = outgoing.get(id) {
            let mut newly = Vec::new();
            for kid in kids {
                if let Some(deps) = incoming.get_mut(kid) {
                    deps.remove(id);
                    if deps.is_empty() {
                        newly.push(*kid);
                    }
                }
            }
            newly.sort_unstable();
            for n in newly {
                queue.push_back(n);
            }
        }
    }

    if order.len() != tasks.len() {
        let leftover: Vec<String> = tasks
            .iter()
            .filter(|t| !order.iter().any(|id| id == &t.id))
            .map(|t| t.id.clone())
            .collect();
        return Err(DagError::Cycle { nodes: leftover });
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const EXAMPLE: &str = r#"
id: 03-mul
files: [src/mul.rs, src/lib.rs]
new: []
verify: "cargo test mul -- --exact"
must_contain: ["pub fn mul"]
depends_on: [01-add]
ctx: 32768
mock: artifacts/foo.png
"#;

    #[test]
    fn happy_parse_of_master_plan_example() {
        let spec = parse_task_yaml(EXAMPLE).unwrap();
        assert_eq!(spec.id, "03-mul");
        assert_eq!(spec.files, vec!["src/mul.rs", "src/lib.rs"]);
        assert_eq!(spec.verify, "cargo test mul -- --exact");
        assert_eq!(spec.must_contain, vec!["pub fn mul"]);
        assert_eq!(spec.depends_on, vec!["01-add"]);
        assert_eq!(spec.ctx, Some(32_768));
        assert_eq!(spec.mock.as_deref(), Some("artifacts/foo.png"));
        assert_eq!(spec.new.as_deref(), Some(&[][..]));
        assert_eq!(spec.cwd, None);
    }

    #[test]
    fn reject_dotdot() {
        let raw = EXAMPLE.replace("src/mul.rs", "../secret");
        let err = parse_task_yaml(&raw).unwrap_err();
        assert!(err.contains(".."), "{err}");
    }

    #[test]
    fn reject_absolute() {
        let raw = EXAMPLE.replace("src/lib.rs", "/etc/passwd");
        let err = parse_task_yaml(&raw).unwrap_err();
        assert!(err.contains("relative") || err.contains("/etc/passwd"), "{err}");
    }

    #[test]
    fn reject_empty_verify() {
        let raw = EXAMPLE.replace("cargo test mul -- --exact", "");
        let err = parse_task_yaml(&raw).unwrap_err();
        assert!(err.contains("verify"), "{err}");
    }

    #[test]
    fn reject_empty_files() {
        let raw = r#"
id: 03-mul
files: []
verify: "cargo test spec -- --exact"
"#;
        let err = parse_task_yaml(raw).unwrap_err();
        assert!(err.contains("files"), "{err}");
    }

    #[test]
    fn reject_new_not_in_files() {
        let raw = r#"
id: 03-mul
files: [src/mul.rs]
new: [src/other.rs]
verify: "cargo test spec -- --exact"
"#;
        let err = parse_task_yaml(raw).unwrap_err();
        assert!(err.contains("new"), "{err}");
    }

    #[test]
    fn reject_empty_id() {
        let raw = r#"
id: ""
files: [src/a.rs]
verify: "cargo test spec -- --exact"
"#;
        let err = parse_task_yaml(raw).unwrap_err();
        assert!(err.contains("empty id"), "{err}");
    }

    #[test]
    fn reject_invalid_id_charset() {
        let raw = r#"
id: "../evil"
files: [src/a.rs]
verify: "cargo test spec -- --exact"
"#;
        let err = parse_task_yaml(raw).unwrap_err();
        assert!(err.contains("invalid task id"), "{err}");
    }

    #[test]
    fn reject_id_mismatch() {
        let dir = tmp_task_dir("01-add");
        std::fs::write(
            dir.join("task.yml"),
            "id: 03-mul\nfiles: [src/a.rs]\nverify: cargo test spec -- --exact\n",
        )
        .unwrap();
        let err = load_task_spec(&dir).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_task_yml_errors() {
        let dir = tmp_task_dir("01-add");
        let err = load_task_spec(&dir).unwrap_err();
        assert!(err.contains("missing task.yml"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_task_spec_accepts_matching_id() {
        let dir = tmp_task_dir("03-mul");
        std::fs::write(dir.join("task.yml"), EXAMPLE).unwrap();
        let spec = load_task_spec(&dir).unwrap();
        assert_eq!(spec.id, "03-mul");
        let _ = std::fs::remove_dir_all(&dir);
    }

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn tmp_task_dir(name: &str) -> std::path::PathBuf {
        let parent = std::env::temp_dir().join(format!(
            "j1-task-spec-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let d = parent.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn spec(id: &str, deps: &[&str]) -> TaskSpec {
        TaskSpec {
            id: id.into(),
            files: vec!["src/x.rs".into()],
            verify: "cargo test spec -- --exact".into(),
            must_contain: vec![],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            ctx: None,
            mock: None,
            new: None,
            cwd: None,
        }
    }

    #[test]
    fn topo_sort_linear() {
        let tasks = [spec("01-a", &[]), spec("02-b", &["01-a"]), spec("03-c", &["02-b"])];
        assert_eq!(topo_sort(&tasks).unwrap(), vec!["01-a", "02-b", "03-c"]);
    }

    #[test]
    fn topo_sort_diamond_is_stable() {
        let tasks = [
            spec("01-a", &[]),
            spec("03-c", &["01-a"]),
            spec("02-b", &["01-a"]),
            spec("04-d", &["02-b", "03-c"]),
        ];
        assert_eq!(
            topo_sort(&tasks).unwrap(),
            vec!["01-a", "02-b", "03-c", "04-d"]
        );
    }

    #[test]
    fn topo_sort_cycle_errors() {
        let tasks = [spec("01-a", &["02-b"]), spec("02-b", &["01-a"])];
        match topo_sort(&tasks) {
            Err(DagError::Cycle { .. }) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn topo_sort_unknown_and_self_dep() {
        assert!(matches!(
            topo_sort(&[spec("01-a", &["nope"])]),
            Err(DagError::UnknownDep { .. })
        ));
        assert!(matches!(
            topo_sort(&[spec("01-a", &["01-a"])]),
            Err(DagError::SelfDep { .. })
        ));
    }

    #[test]
    fn topo_sort_empty_ok() {
        assert!(topo_sort(&[]).unwrap().is_empty());
    }

    #[test]
    fn cwd_is_optional_and_jailed() {
        let spec = parse_task_yaml(EXAMPLE).unwrap();
        assert_eq!(spec.cwd, None);
        let with_cwd = EXAMPLE.replace("ctx: 32768", "ctx: 32768\ncwd: desktop/src-tauri");
        let spec = parse_task_yaml(&with_cwd).unwrap();
        assert_eq!(spec.cwd.as_deref(), Some("desktop/src-tauri"));
        let bad = EXAMPLE.replace("ctx: 32768", "ctx: 32768\ncwd: ../secret");
        let err = parse_task_yaml(&bad).unwrap_err();
        assert!(err.contains(".."), "{err}");
    }
}

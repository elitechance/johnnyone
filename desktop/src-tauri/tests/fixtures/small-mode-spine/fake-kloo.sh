#!/bin/sh
# Deterministic kloo stand-in for default cargo test. No network.
# Copies --allow files from J1_FAKE_KLOO_EXPECTED (workspace-expected) and
# runs --verify. Prints one KLOO_RESULT_JSON line on stdout.
#
# Constraint: --allow paths must not contain whitespace (word-split on
# $allows) and --verify must not contain double quotes (interpolated
# unescaped into KLOO_RESULT_JSON). The committed fixture satisfies both.
set -eu

EXPECTED="${J1_FAKE_KLOO_EXPECTED:-}"
TASKS_JSON="${J1_FAKE_KLOO_TASKS_JSON:-}"
MODE="${J1_FAKE_KLOO_MODE:-copy}"
SLEEP_SECS="${J1_FAKE_KLOO_SLEEP_SECS:-0}"
SLEEP_ALLOW="${J1_FAKE_KLOO_SLEEP_ALLOW:-}"
PID_FILE="${J1_FAKE_KLOO_PID_FILE:-}"
MODEL_LOG="${J1_FAKE_KLOO_MODEL_LOG:-}"
FORCE_FAIL="${J1_FAKE_KLOO_FORCE_FAIL:-}"
VERIFY_EXIT_JSON="${J1_FAKE_KLOO_VERIFY_EXIT_JSON:-}"
FAILURE_CODE="${J1_FAKE_KLOO_FAILURE_CODE:-verify_failed}"
POSTCHECK_FAIL="${J1_FAKE_KLOO_POSTCHECK_FAIL:-}"
NO_REWRITE="${J1_FAKE_KLOO_NO_REWRITE:-}"

allows=""
verify=""
model=""
benchmark=0
pending=""
for arg in "$@"; do
  if [ -n "$pending" ]; then
    case "$pending" in
      allow) allows="$allows $arg" ;;
      verify) verify="$arg" ;;
      model) model="$arg" ;;
    esac
    pending=""
    continue
  fi
  case "$arg" in
    --benchmark) benchmark=1 ;;
    --allow) pending=allow ;;
    --verify) pending=verify ;;
    --model) pending=model ;;
    --) break ;;
    --*) ;;
    *) ;;
  esac
done

if [ "$benchmark" != 1 ]; then
  echo "fake-kloo: --benchmark required" >&2
  exit 2
fi

# Infer task id from first allow path (src/add.rs → 01-add via TASK_MAP or basename).
first_allow=""
for a in $allows; do
  first_allow="$a"
  break
done

task_id=""
case "$first_allow" in
  src/add.rs) task_id=01-add ;;
  src/sub.rs) task_id=02-sub ;;
  src/mul.rs) task_id=03-mul ;;
  src/div.rs) task_id=04-div ;;
  src/clamp.rs) task_id=05-clamp ;;
  src/saturate.rs) task_id=06-saturate ;;
  src/gcd.rs) task_id=07-gcd ;;
  src/lcm.rs) task_id=08-lcm ;;
  src/min.rs) task_id=09-min ;;
  src/max.rs) task_id=10-max ;;
  *) task_id=$(basename "${first_allow:-unknown}" .rs) ;;
esac

if [ -n "${J1_FAKE_KLOO_ID_MAP:-}" ] && [ -f "$J1_FAKE_KLOO_ID_MAP" ]; then
  mapped=$(awk -F= -v p="$first_allow" '$1==p { print $2; exit }' "$J1_FAKE_KLOO_ID_MAP")
  if [ -n "$mapped" ]; then
    task_id="$mapped"
  fi
fi

if [ -n "$MODEL_LOG" ]; then
  printf '%s\t%s\n' "$task_id" "$model" >> "$MODEL_LOG"
fi

if [ -n "$TASKS_JSON" ] && [ "${J1_FAKE_KLOO_ASSERT_RUNNING:-}" = "1" ]; then
  if [ ! -f "$TASKS_JSON" ]; then
    echo "fake-kloo: tasks.json missing at $TASKS_JSON" >&2
    exit 10
  fi
  if ! grep -q "\"id\": \"$task_id\"" "$TASKS_JSON" && ! grep -q "\"id\":\"$task_id\"" "$TASKS_JSON"; then
    echo "fake-kloo: task $task_id not in tasks.json" >&2
    exit 10
  fi
  # Pretty JSON from serde: after the matching "id" line the next "status"
  # belongs to that row. Compact form is also accepted. POSIX awk only.
  if ! awk -v tid="$task_id" '
    $0 ~ "\"id\"[[:space:]]*:[[:space:]]*\"" tid "\"" { hit=1; next }
    hit && /"status"/ {
      exit !($0 ~ /"running"/)
    }
    END { if (!hit) exit 1 }
  ' "$TASKS_JSON"
  then
    echo "fake-kloo: task $task_id is not status=running in $TASKS_JSON" >&2
    exit 10
  fi
fi

if [ -n "$SLEEP_ALLOW" ] && [ "$first_allow" = "$SLEEP_ALLOW" ] && [ "$SLEEP_SECS" != "0" ]; then
  if [ -n "$PID_FILE" ]; then
    echo $$ > "$PID_FILE"
  fi
  sleep "$SLEEP_SECS"
fi

copied=""
if [ "$MODE" != "no_copy" ] && [ -n "$EXPECTED" ]; then
  for a in $allows; do
    src="$EXPECTED/$a"
    if [ ! -f "$src" ]; then
      echo "fake-kloo: expected missing $src" >&2
      exit 21
    fi
    # Refuse to write outside allow (already iterating allow list).
    case "$a" in
      ..*|/*) echo "fake-kloo: allow path outside workspace: $a" >&2; exit 21 ;;
    esac
    if [ "$NO_REWRITE" = "1" ] && [ -f "$a" ]; then
      if cmp -s "$src" "$a"; then
        continue
      fi
    fi
    mkdir -p "$(dirname "$a")"
    cp "$src" "$a"
    copied="$copied $a"
  done
fi

json_paths=""
sep=""
for a in $copied; do
  json_paths="${json_paths}${sep}\"${a}\""
  sep=", "
done
count=0
for a in $copied; do
  count=$((count + 1))
done

verify_exit=0
if [ -n "$verify" ]; then
  sh -c "$verify" >/dev/null 2>&1 || verify_exit=$?
fi

if [ -n "$FORCE_FAIL" ]; then
  verify_exit="${VERIFY_EXIT_JSON:-1}"
  ve="${VERIFY_EXIT_JSON:-1}"
  success=false
  fc="$FAILURE_CODE"
  proc_exit=10
  if [ -n "$POSTCHECK_FAIL" ]; then
    printf 'KLOO_RESULT_JSON {"success":false,"failure_code":"postcheck_failed","files_changed":{"count":%s,"paths":[%s]},"off_scope_edits":0,"verify":{"command":"%s","passed":true,"exit_code":0},"postchecks":[{"command":"lint","passed":false,"exit_code":1}]}\n' \
      "$count" "$json_paths" "$verify"
    exit 10
  fi
  printf 'KLOO_RESULT_JSON {"success":false,"failure_code":"%s","files_changed":{"count":%s,"paths":[%s]},"off_scope_edits":0,"verify":{"command":"%s","passed":false,"exit_code":%s}}\n' \
    "$fc" "$count" "$json_paths" "$verify" "$ve"
  exit 10
fi

if [ "$verify_exit" -eq 0 ]; then
  printf 'KLOO_RESULT_JSON {"success":true,"files_changed":{"count":%s,"paths":[%s]},"off_scope_edits":0,"verify":{"command":"%s","passed":true,"exit_code":0}}\n' \
    "$count" "$json_paths" "$verify"
  exit 0
fi

printf 'KLOO_RESULT_JSON {"success":false,"failure_code":"verify_failed","files_changed":{"count":%s,"paths":[%s]},"off_scope_edits":0,"verify":{"command":"%s","passed":false,"exit_code":1}}\n' \
  "$count" "$json_paths" "$verify"
exit 10

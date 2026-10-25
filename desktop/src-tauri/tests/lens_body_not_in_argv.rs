//! The lens request body must not travel in argv.
//!
//! THE DEFECT THIS PINS, and it was mine.
//!
//! G7 raised `LENS_MATERIAL_BUDGET_BYTES` from 48,000 to 160,000 because the
//! budget was cutting `review-lenses.md` at 63% and dropping `ui.md` entirely.
//! The arithmetic in that comment is about the model's CONTEXT WINDOW — 160,000
//! chars ÷ 3.5 ≈ 45,714 tokens of a configured 131,072-token window — and it is
//! correct. It is also the wrong constraint.
//!
//! `lokalai::build_config` puts the whole JSON body in one argv string
//! (`"--data", body`). Linux caps a SINGLE argv string at `MAX_ARG_STRLEN`,
//! which is 32 pages — 131,072 bytes — independently of the much larger
//! `ARG_MAX` total (2,097,152 here). Measured on this box, 2026-10-25:
//!
//! ```text
//!   120,000 bytes in one argv string: spawned OK
//!   131,000 bytes in one argv string: spawned OK
//!   131,072 bytes in one argv string: OSError errno=7 Argument list too long
//!   160,000 bytes in one argv string: OSError errno=7 Argument list too long
//! ```
//!
//! So the moment the material actually filled its new budget, `spawn` failed
//! before curl ever ran. kart's phase `01-foundation` passed all four tasks and
//! then died on its review with:
//!
//! ```text
//!   coordinator_failed: Failed to spawn 'curl': Argument list too long (os error 7)
//! ```
//!
//! G7's own gate had a ceiling of 300,000 bytes. It was there to catch a
//! runaway raise, and it was derived from the token window, so it would have
//! waved 160,000 through — and did. A rail whose number comes from the wrong
//! physical limit is not a rail.
//!
//! The fix is NOT a smaller budget. A smaller budget leaves the cliff in place,
//! one phase of material away, and brings back the truncation G7 removed.
//! `a4` exists to make that explicit: it sends THREE TIMES the budget and
//! requires it to arrive. Only moving the body off argv — `--data @-` plus the
//! body on the child's stdin — can satisfy a1 and a4 together.

use johnnyone_desktop_lib::providers::lokalai;
use johnnyone_desktop_lib::services::stream_lens::{run_stream_lens, LENS_MATERIAL_BUDGET_BYTES};

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

const SSE_PASS: &str = concat!(
    "data: {\"choices\":[{\"delta\":{\"content\":\"Everything in phase 01 is present. \"}}]}\n\n",
    "data: {\"choices\":[{\"delta\":{\"content\":\"VERDICT: PASS\"}}]}\n\n",
    "data: [DONE]\n\n",
);

/// Serve one request and keep the WHOLE body.
///
/// It reads headers, parses `Content-Length`, then reads exactly that many body
/// bytes. The sibling helper in `stream_lens_material.rs` stops after one short
/// read past the headers, which is fine for a small prompt and would silently
/// truncate the 160KB and 480KB bodies these tests are entirely about — a
/// capture that truncates turns a4 into a test of the helper.
fn serve_once_capturing_whole_body(reply: &'static str) -> (u16, Arc<Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a throwaway port");
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(String::new()));
    let sink = Arc::clone(&seen);
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the lens must connect");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .ok();
        let mut all: Vec<u8> = Vec::new();
        let mut buf = [0u8; 65536];

        // 1. headers
        let header_end = loop {
            match stream.read(&mut buf) {
                Ok(0) => break None,
                Ok(n) => {
                    all.extend_from_slice(&buf[..n]);
                    if let Some(i) = all.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                }
                Err(_) => break None,
            }
        };

        // 2. exactly Content-Length body bytes — never a fixed number of reads
        if let Some(start) = header_end {
            let headers = String::from_utf8_lossy(&all[..start]).to_lowercase();
            let want: usize = headers
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            while all.len() - start < want {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => all.extend_from_slice(&buf[..n]),
                    Err(_) => break,
                }
            }
        }

        *sink.lock().unwrap() = String::from_utf8_lossy(&all).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });
    (port, seen)
}

/// A prompt of `n` bytes carrying `canary` where the wire can be checked for it.
fn prompt_of(n: usize, canary: &str) -> String {
    let mut s = String::with_capacity(n + canary.len());
    s.push_str(canary);
    s.push('\n');
    while s.len() < n {
        s.push_str("CONVENTIONS: every colour in the app comes from one token file. ");
    }
    s.truncate(n.max(canary.len() + 1));
    s
}

/// a1 — the body is on stdin, and nothing oversized is left in argv.
///
/// The 100,000-byte assertion is deliberately BELOW the 131,072 kernel limit:
/// a config that merely squeaks under the cliff is still one phase of material
/// away from it, and this test must fail on that rather than bless it.
#[test]
fn a1_build_config_keeps_the_body_out_of_argv() {
    let prompt = prompt_of(LENS_MATERIAL_BUDGET_BYTES, "CANARY-A1");
    let config = lokalai::build_config(
        &prompt,
        "/tmp",
        "glm",
        "http://127.0.0.1:1/v1",
        "sk-not-in-argv",
    );

    let worst = config.args.iter().map(|a| a.len()).max().unwrap_or(0);
    assert!(
        worst < 100_000,
        "the longest argv string is {worst} bytes. MAX_ARG_STRLEN is 131_072, so this config \
         fails to spawn at all once the material fills its budget — the failure is E2BIG from \
         `spawn`, before curl runs, and it surfaces as `coordinator_failed`, not as a lens error."
    );

    let data_value = config
        .args
        .iter()
        .position(|a| a == "--data" || a == "--data-binary")
        .and_then(|i| config.args.get(i + 1))
        .map(String::as_str);
    assert_eq!(
        data_value,
        Some("@-"),
        "the --data value must be the literal `@-` so curl reads the body from stdin; \
         args were: {:?}",
        config.args.iter().map(|a| a.len()).collect::<Vec<_>>()
    );

    let body = config
        .stdin
        .as_deref()
        .expect("the body must be handed to the child on stdin");
    assert!(
        body.contains("CANARY-A1"),
        "the prompt did not reach the stdin body at all"
    );
    assert!(
        body.len() >= LENS_MATERIAL_BUDGET_BYTES,
        "the stdin body is {} bytes for a {LENS_MATERIAL_BUDGET_BYTES}-byte prompt — \
         something truncated it",
        body.len()
    );
}

/// a2 — THE REGRESSION. A full-budget prompt must reach the provider and come
/// back as a verdict. This is the exact shape that killed kart's phase review.
#[tokio::test]
async fn a2_a_full_budget_prompt_reaches_the_provider() {
    let (port, seen) = serve_once_capturing_whole_body(SSE_PASS);
    let prompt = prompt_of(LENS_MATERIAL_BUDGET_BYTES, "CANARY-A2-FULL-BUDGET");

    let report = run_stream_lens(
        &format!("http://127.0.0.1:{port}/v1"),
        "sk-test-key-not-in-argv",
        "glm",
        &prompt,
        "/tmp",
        "a2-full-budget-has-no-pane",
        "glm-validator",
    )
    .await
    .expect(
        "a prompt at exactly LENS_MATERIAL_BUDGET_BYTES must spawn. If this is \
         `Argument list too long (os error 7)`, the body is still in argv.",
    );

    assert_eq!(report.verdict.as_deref(), Some("PASS"));
    let wire = seen.lock().unwrap().clone();
    assert!(
        wire.contains("CANARY-A2-FULL-BUDGET"),
        "the lens returned a verdict but the prompt never reached the wire ({} bytes seen)",
        wire.len()
    );
}

/// a3 — the control arm. A small prompt must still work, so the fix cannot be
/// "only the big path is handled", and what the config says it will send must be
/// what the socket actually receives.
#[tokio::test]
async fn a3_a_small_prompt_still_works_and_matches_the_config() {
    let (port, seen) = serve_once_capturing_whole_body(SSE_PASS);
    let prompt = "CANARY-A3 Review the kart catalog page.";
    let endpoint = format!("http://127.0.0.1:{port}/v1");

    let config = lokalai::build_config(prompt, "/tmp", "glm", &endpoint, "sk-test-key");
    let planned = config
        .stdin
        .as_deref()
        .expect("even a small body goes on stdin — one path, not two")
        .to_string();

    let report = run_stream_lens(
        &endpoint,
        "sk-test-key-not-in-argv",
        "glm",
        prompt,
        "/tmp",
        "a3-small-prompt-has-no-pane",
        "glm-validator",
    )
    .await
    .expect("a small prompt must still return a report");

    assert_eq!(report.verdict.as_deref(), Some("PASS"));
    let wire = seen.lock().unwrap().clone();
    assert!(
        wire.contains("CANARY-A3"),
        "the small prompt did not reach the wire"
    );
    assert!(
        wire.contains(&planned),
        "the body on the wire is not the body the config said it would send — \
         stdin and argv have diverged"
    );
}

/// a4 — the cliff must be GONE, not merely further away.
///
/// Three times the budget. A fix that lowers `LENS_MATERIAL_BUDGET_BYTES` under
/// 131,072 passes a2 and fails here, which is the point: that fix keeps the
/// cliff and re-introduces the truncation G7 removed. Only taking the body out
/// of argv passes both.
#[tokio::test]
async fn a4_three_times_the_budget_still_arrives() {
    let (port, seen) = serve_once_capturing_whole_body(SSE_PASS);
    let huge = LENS_MATERIAL_BUDGET_BYTES * 3;
    let prompt = prompt_of(huge, "CANARY-A4-THREE-X");

    let report = run_stream_lens(
        &format!("http://127.0.0.1:{port}/v1"),
        "sk-test-key-not-in-argv",
        "glm",
        &prompt,
        "/tmp",
        "a4-three-x-budget-has-no-pane",
        "glm-validator",
    )
    .await
    .unwrap_or_else(|e| {
        panic!(
            "{huge} bytes must reach the provider: argv is no longer supposed to be \
             the constraint on prompt size. Got: {e}"
        )
    });

    assert_eq!(report.verdict.as_deref(), Some("PASS"));
    let wire = seen.lock().unwrap().clone();
    assert!(
        wire.contains("CANARY-A4-THREE-X"),
        "{huge} bytes spawned but the prompt never reached the wire ({} bytes seen)",
        wire.len()
    );
}

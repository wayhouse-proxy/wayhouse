//! Plugin port routes under `--plugins` with `--ha-peers` (#403, #422): the host assigns
//! entry ports in the replicated apply step, every replica serves the same table, a backend
//! change for the same key keeps the port, a leader failover keeps every port, and a plugin
//! whose approved ports are all taken is told so instead of being dropped silently. (UDP to a
//! tunnel backend is the existing `tcp_and_udp_round_trip_through_the_tunnel` scenario: a
//! `protocol: udp` listener in front of a tunnel-source pool.) Plain `cargo test`.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use wayhouse_fleet_tests::{
    build_fleet_bins, free_port, spawn_controller_with, wait_http_up, wait_until, Proc,
};

const AUTH_TOKEN: &str = "ha-port-routes-test-client-token";

fn caps(ports: &str) -> String {
    format!(
        r#"{{"triggers":{{"on_timer":true}},"tick_interval_secs":10,"log":true,"state":{{"max_bytes":64}},"port_routes":{{"ports":["{ports}"],"proto":["udp","tcp"],"backends":["10.0.0.0/16"],"max_entries":4}}}}"#
    )
}

/// Declares one of two sets of port routes on every tick, `flip` choosing the set by the
/// parity of a tick counter kept in state: the two sets differ in the backend of key `k1`
/// only, so with `flip` the plugin moves `k1` between two backends every tick.
fn module(caps: &str, sets: [&str; 2], flip: bool) -> Vec<u8> {
    assert_eq!(sets[0].len(), sets[1].len(), "the guest passes one length");
    let esc = |s: &str| s.replace('"', "\\\"");
    let (caps, a, b) = (esc(caps), esc(sets[0]), esc(sets[1]));
    let len = sets[0].len();
    let select = if flip {
        "(i32.add (i32.const 300) (i32.mul (i32.const 1000) (i32.and (i32.load8_u (i32.const 100)) (i32.const 1))))"
    } else {
        "(i32.const 300)"
    };
    wat::parse_str(format!(
        r#"(module
  (@custom "wayhouse.plugin-abi" "\00\00\01\00")
  (@custom "wayhouse.plugin-caps" "{caps}")
  (import "wayhouse" "state_get" (func $get (param i32 i32 i32 i32) (result i32)))
  (import "wayhouse" "state_put" (func $put (param i32 i32 i32 i32) (result i32)))
  (import "wayhouse" "port_routes_set" (func $prs (param i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "n")
  (data (i32.const 300) "{a}")
  (data (i32.const 1300) "{b}")
  (global $bump (mut i32) (i32.const 4096))
  (func (export "alloc") (param i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get 0)))
    (local.get $p))
  (func (export "init") (param i32 i32))
  (func (export "on_timer")
    (drop (call $get (i32.const 16) (i32.const 1) (i32.const 100) (i32.const 1)))
    (i32.store8 (i32.const 100) (i32.add (i32.load8_u (i32.const 100)) (i32.const 1)))
    (drop (call $put (i32.const 16) (i32.const 1) (i32.const 100) (i32.const 1)))
    (drop (call $prs {select} (i32.const {len})))))"#
    ))
    .expect("the port route module assembles")
}

async fn leader_id(http: &reqwest::Client, base: &str) -> Option<u64> {
    let r = http
        .get(format!("{base}/admin/ha/members"))
        .bearer_auth(AUTH_TOKEN)
        .send()
        .await
        .ok()?;
    r.json::<Value>().await.ok()?["leader"].as_u64()
}

async fn overlay(http: &reqwest::Client, base: &str) -> Result<Value> {
    Ok(http
        .get(format!("{base}/plugin-routes"))
        .bearer_auth(AUTH_TOKEN)
        .send()
        .await?
        .json()
        .await?)
}

/// `key -> (port, backend)` of an overlay's port routes.
fn table(overlay: &Value) -> BTreeMap<String, (u64, String)> {
    overlay["port_routes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| {
            (
                r["key"].as_str().unwrap_or_default().to_string(),
                (
                    r["port"].as_u64().unwrap_or_default(),
                    r["backend"].as_str().unwrap_or_default().to_string(),
                ),
            )
        })
        .collect()
}

async fn install(
    http: &reqwest::Client,
    base: &str,
    name: &str,
    caps: &str,
    wasm: Vec<u8>,
) -> Result<()> {
    let up = http
        .post(format!("{base}/plugins/modules"))
        .bearer_auth(AUTH_TOKEN)
        .body(wasm)
        .send()
        .await?;
    ensure!(up.status() == 201, "upload: {}", up.status());
    let sha = up.json::<Value>().await?["sha256"]
        .as_str()
        .context("sha256")?
        .to_string();
    let approved: Value = serde_json::from_str(caps)?;
    let r = http
        .post(format!("{base}/plugins"))
        .bearer_auth(AUTH_TOKEN)
        .json(&json!({"name": name, "sha256": sha, "approved": approved}))
        .send()
        .await?;
    ensure!(r.status() == 201, "install {name}: {}", r.status());
    Ok(())
}

#[tokio::test]
async fn entry_ports_are_stable_across_moves_and_failover_and_exhaustion_is_reported() -> Result<()>
{
    build_fleet_bins()?;
    let http = wayhouse_http::client();
    let ports = (0..3).map(|_| free_port()).collect::<Result<Vec<_>>>()?;
    let peers = ports
        .iter()
        .enumerate()
        .map(|(i, p)| format!("{}=127.0.0.1:{p}", i + 1))
        .collect::<Vec<_>>()
        .join(",");
    let bases: Vec<String> = ports
        .iter()
        .map(|p| format!("http://127.0.0.1:{p}"))
        .collect();
    let mut procs: Vec<Option<Proc>> = Vec::new();
    let mut dirs = Vec::new();
    for (i, p) in ports.iter().enumerate() {
        let dir = tempfile::tempdir()?;
        let extra = [
            "--ha-node-id",
            &(i + 1).to_string(),
            "--ha-peers",
            &peers,
            "--auth-token",
            AUTH_TOKEN,
            "--tunnel-network",
            "10.62.0.0/24",
            "--plugins",
        ]
        .map(String::from);
        procs.push(Some(spawn_controller_with(
            dir.path(),
            &format!("127.0.0.1:{p}"),
            &extra,
        )?));
        dirs.push(dir);
        wait_http_up(
            &format!("{}/healthz", bases[i]),
            wayhouse_fleet_tests::STARTUP_TIMEOUT,
        )
        .await?;
    }
    wait_until(
        || async {
            let mut seen = Vec::new();
            for b in &bases {
                seen.push(leader_id(&http, b).await);
            }
            Ok(seen[0].is_some() && seen.iter().all(|s| *s == seen[0]))
        },
        Duration::from_secs(20),
        "all nodes agree on a leader",
    )
    .await?;
    let leader = leader_id(&http, &bases[0]).await.context("a leader")? as usize - 1;
    let follower = (leader + 1) % 3;

    // Plugin A: k1 prefers 30005 and moves between two backends every tick; k2 takes the
    // lowest free port of 30000-30009.
    let set = |k1: &str| {
        format!(
            r#"[{{"key":"k1","backend":"{k1}","proto":"udp","prefer":30005}},{{"key":"k2","backend":"10.0.1.3:25565","proto":"both"}}]"#
        )
    };
    let caps_a = caps("30000-30009");
    install(
        &http,
        &bases[follower],
        "movers",
        &caps_a,
        module(
            &caps_a,
            [&set("10.0.1.2:7777"), &set("10.0.1.2:7778")],
            true,
        ),
    )
    .await?;
    let placed = |t: &BTreeMap<String, (u64, String)>| {
        t.get("k1").map(|v| v.0) == Some(30005) && t.get("k2").map(|v| v.0) == Some(30000)
    };
    for base in &bases {
        wait_until(
            || async { Ok(placed(&table(&overlay(&http, base).await?))) },
            Duration::from_secs(60),
            "every replica serves k1 on its preferred port and k2 on the lowest free one",
        )
        .await?;
    }

    // A backend change for the same key keeps the port.
    let backends = std::sync::Mutex::new(std::collections::BTreeSet::new());
    wait_until(
        || async {
            let t = table(&overlay(&http, &bases[follower]).await?);
            let (port, backend) = t.get("k1").cloned().context("k1")?;
            ensure!(port == 30005, "k1 moved to port {port}");
            let mut seen = backends.lock().unwrap();
            seen.insert(backend);
            Ok(seen.len() == 2)
        },
        Duration::from_secs(60),
        "k1 changed its backend while keeping port 30005",
    )
    .await?;

    // Plugin B is approved for two ports, one already taken by k2: one key fits, two wait.
    let caps_b = caps("30000-30001");
    let doc_b = r#"[{"key":"kb1","backend":"10.0.2.1:1","proto":"tcp"},{"key":"kb2","backend":"10.0.2.2:1","proto":"tcp"},{"key":"kb3","backend":"10.0.2.3:1","proto":"tcp"}]"#;
    install(
        &http,
        &bases[follower],
        "crowded",
        &caps_b,
        module(&caps_b, [doc_b, doc_b], false),
    )
    .await?;
    for base in &bases {
        wait_until(
            || async {
                let o = overlay(&http, base).await?;
                Ok(o["exhausted"].as_array().is_some_and(|e| e.len() == 2)
                    && table(&o).get("kb1").map(|v| v.0) == Some(30001))
            },
            Duration::from_secs(60),
            "kb1 got the one free port and kb2, kb3 are reported as waiting",
        )
        .await?;
    }

    // Kill the leader: the new one keeps every assigned port.
    let before: BTreeMap<String, u64> = table(&overlay(&http, &bases[follower]).await?)
        .into_iter()
        .map(|(k, (p, _))| (k, p))
        .collect();
    ensure!(before.len() == 3, "{before:?}");
    procs[leader].take().context("leader proc")?.kill().await?;
    wait_until(
        || async {
            let l = leader_id(&http, &bases[follower]).await;
            Ok(l.is_some_and(|l| l as usize - 1 != leader))
        },
        Duration::from_secs(30),
        "a new leader is elected",
    )
    .await?;
    // Let the new leader tick a few times, then compare.
    tokio::time::sleep(Duration::from_secs(35)).await;
    for i in [follower, 3 - leader - follower] {
        let after: BTreeMap<String, u64> = table(&overlay(&http, &bases[i]).await?)
            .into_iter()
            .map(|(k, (p, _))| (k, p))
            .collect();
        ensure!(
            after == before,
            "ports moved on node {i}: {before:?} -> {after:?}"
        );
    }
    drop(procs);
    Ok(())
}

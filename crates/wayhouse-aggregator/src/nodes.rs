//! The node list behind `GET /fleet/nodes`: filtered, sorted, grouped by region and paged on the
//! server, so a fleet of hundreds of nodes never travels to the browser whole. The same filter
//! backs `POST /fleet/nodes/resolve`, which a bulk action calls to learn which instances (and how
//! many) a filter selects before the operator confirms.
//!
//! Tags and region are what an instance reports about itself (`--region`, `--tag key=value`); the
//! aggregator stores nothing of its own, like the rest of this crate. Tags an operator set live in
//! the controller; the UI backend sends them with each query ([`parse_operator`], [`overlay`]) and
//! they win over a declared tag of the same key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use wayhouse_http::labels::{check_labels, key_ok, MAX_TAGS};

use crate::ingest::{InstanceState, SessionCounts};
use crate::skew::{classify, Reported, Skew};

/// Most rows one page holds.
pub const MAX_PAGE_SIZE: usize = 200;
const DEFAULT_PAGE_SIZE: usize = 25;
/// Longest free-text filter.
const MAX_QUERY: usize = 128;

/// A node's health at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    /// Pushing and every backend healthy.
    Ok,
    /// Pushing, at least one backend unhealthy.
    Degraded,
    /// Not heard from within the stale threshold.
    Stale,
}

impl NodeState {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Self::Ok),
            "degraded" => Some(Self::Degraded),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortKey {
    Instance,
    State,
    Region,
    Sessions,
    Version,
}

/// Which nodes a request selects. All parts must match.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Filter {
    /// Case-insensitive substring of the instance, region, group, version or a `key=value` tag.
    q: Option<String>,
    state: Option<NodeStateFilter>,
    region: Option<String>,
    /// `(key, value)`: the instance carries the tag, with that value when one is given.
    tags: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NodeStateFilter(NodeState);

/// A parsed `GET /fleet/nodes` query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pub filter: Filter,
    page: usize,
    size: usize,
    sort: SortKey,
    desc: bool,
    group_by_region: bool,
}

impl Filter {
    fn parse_pair(&mut self, key: &str, value: &str) -> Result<bool, String> {
        match key {
            "q" if value.len() > MAX_QUERY => {
                return Err(format!("q is longer than {MAX_QUERY} bytes"))
            }
            "q" if !value.is_empty() => self.q = Some(value.to_lowercase()),
            "state" => {
                let s = NodeState::parse(value).ok_or("state must be ok, degraded or stale")?;
                self.state = Some(NodeStateFilter(s));
            }
            "region" if !value.is_empty() => self.region = Some(value.to_string()),
            "tag" if !value.is_empty() => {
                let (k, v) = match value.split_once('=') {
                    Some((k, v)) => (k, Some(v.to_string())),
                    None => (value, None),
                };
                if !key_ok(k) {
                    return Err(format!("tag filter {value:?}: not a tag key"));
                }
                if self.tags.len() >= MAX_TAGS {
                    return Err(format!("more than {MAX_TAGS} tag filters"));
                }
                self.tags.push((k.to_string(), v));
            }
            "q" | "region" | "tag" => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// The filter part of a query string (`q`, `state`, `region`, repeated `tag`). Other keys are
    /// ignored, so `resolve` accepts the very query a list view sent.
    pub fn parse(query: &str) -> Result<Self, String> {
        let mut f = Filter::default();
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            f.parse_pair(&k, &v)?;
        }
        Ok(f)
    }

    fn matches(&self, n: &Node) -> bool {
        self.state.is_none_or(|s| s.0 == n.state)
            && self
                .region
                .as_ref()
                .is_none_or(|r| n.region.as_ref() == Some(r))
            && self.tags.iter().all(|(k, v)| {
                n.tags
                    .get(k)
                    .is_some_and(|have| v.as_ref().is_none_or(|v| v == have))
            })
            && self.q.as_ref().is_none_or(|q| {
                let hay = |s: &str| s.to_lowercase().contains(q.as_str());
                hay(&n.instance)
                    || n.region.as_deref().is_some_and(hay)
                    || n.group.as_deref().is_some_and(hay)
                    || hay(&n.version)
                    || n.tags.iter().any(|(k, v)| hay(&format!("{k}={v}")))
            })
    }
}

impl Query {
    /// `page` (from 1), `size` (1 to 200, default 25), `sort` (`instance`, `state`, `region`,
    /// `sessions`, `version`), `dir` (`asc`, `desc`), `group_by=region`, and the [`Filter`] keys.
    pub fn parse(query: &str) -> Result<Self, String> {
        let mut q = Query {
            filter: Filter::default(),
            page: 1,
            size: DEFAULT_PAGE_SIZE,
            sort: SortKey::Instance,
            desc: false,
            group_by_region: false,
        };
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            if q.filter.parse_pair(&k, &v)? {
                continue;
            }
            match &*k {
                "page" => {
                    q.page = v
                        .parse()
                        .ok()
                        .filter(|p| *p >= 1)
                        .ok_or("page must be 1 or more")?
                }
                "size" => {
                    q.size = v
                        .parse()
                        .ok()
                        .filter(|s| (1..=MAX_PAGE_SIZE).contains(s))
                        .ok_or_else(|| format!("size must be 1 to {MAX_PAGE_SIZE}"))?;
                }
                "sort" => {
                    q.sort = match &*v {
                        "instance" => SortKey::Instance,
                        "state" => SortKey::State,
                        "region" => SortKey::Region,
                        "sessions" => SortKey::Sessions,
                        "version" => SortKey::Version,
                        _ => {
                            return Err(
                                "sort must be instance, state, region, sessions or version".into()
                            )
                        }
                    }
                }
                "dir" => {
                    q.desc = match &*v {
                        "asc" => false,
                        "desc" => true,
                        _ => return Err("dir must be asc or desc".into()),
                    }
                }
                "group_by" => match &*v {
                    "region" => q.group_by_region = true,
                    "" => {}
                    _ => return Err("group_by must be region".into()),
                },
                _ => {}
            }
        }
        Ok(q)
    }
}

/// One row of the node list.
#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub instance: String,
    pub state: NodeState,
    pub last_seen_ms_ago: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Declared tags with the operator's laid over them.
    pub tags: BTreeMap<String, String>,
    /// The part of `tags` an operator set (the tag editor shows these as editable).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub operator_tags: BTreeMap<String, String>,
    pub version: String,
    pub protocol: String,
    pub skew: Skew,
    pub sessions: SessionCounts,
    pub backends: usize,
    pub unhealthy_backends: usize,
}

/// Counts over a set of nodes.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct Rollup {
    pub nodes: usize,
    pub degraded: usize,
    pub stale: usize,
    pub sessions: u64,
}

impl Rollup {
    fn add(&mut self, n: &Node) {
        self.nodes += 1;
        self.degraded += usize::from(n.state == NodeState::Degraded);
        self.stale += usize::from(n.state == NodeState::Stale);
        self.sessions += (n.sessions.tcp + n.sessions.udp) as u64;
    }
}

#[derive(Debug, Serialize)]
pub struct Group {
    /// The region; `null` for the nodes that report none.
    pub region: Option<String>,
    #[serde(flatten)]
    pub rollup: Rollup,
}

#[derive(Debug, Serialize)]
pub struct Page {
    pub items: Vec<Node>,
    /// Nodes the filter selects, across all pages.
    pub total: usize,
    pub page: usize,
    pub size: usize,
    /// Over every node the filter selects, not just this page.
    pub rollup: Rollup,
    /// With `group_by=region`: one rollup per region of the selection, regions ascending, the
    /// regionless group last. The list is then ordered by region first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<Group>>,
}

/// Every node as the list shows it, instance ascending; `stale_after_ms` is the silence after
/// which a node is [`NodeState::Stale`].
pub fn nodes(states: &[InstanceState], now_ms: u64, stale_after_ms: u64) -> Vec<Node> {
    let skews = classify(
        &states
            .iter()
            .map(|s| Reported {
                version: &s.payload.version,
                protocol: &s.payload.protocol,
                mismatch_rose_ms_ago: s.mismatch_rose_at_ms.map(|t| now_ms.saturating_sub(t)),
                fresh: now_ms.saturating_sub(s.received_at_ms) <= stale_after_ms,
            })
            .collect::<Vec<_>>(),
    );
    states
        .iter()
        .zip(skews)
        .map(|(s, skew)| {
            let age = now_ms.saturating_sub(s.received_at_ms);
            let backends = s.payload.pools.iter().flat_map(|p| &p.backends);
            let (total, unhealthy) =
                backends.fold((0, 0), |(t, u), b| (t + 1, u + usize::from(!b.healthy)));
            let state = if age > stale_after_ms {
                NodeState::Stale
            } else if unhealthy > 0 {
                NodeState::Degraded
            } else {
                NodeState::Ok
            };
            Node {
                instance: s.payload.instance.clone(),
                state,
                last_seen_ms_ago: age,
                region: s.payload.region.clone(),
                group: s.payload.group.clone(),
                tags: s.payload.tags.clone(),
                operator_tags: BTreeMap::new(),
                version: s.payload.version.clone(),
                protocol: s.payload.protocol.clone(),
                skew,
                sessions: s.payload.sessions,
                backends: total,
                unhealthy_backends: unhealthy,
            }
        })
        .collect()
}

/// Operator-set tags by instance id.
pub type OperatorTags = BTreeMap<String, BTreeMap<String, String>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OverlayBody {
    operator_tags: OperatorTags,
}

/// The body of a query that carries operator tags: `{"operator_tags": {"<instance>": {"k": "v"}}}`,
/// every tag held to the same rules as a declared one. An empty body means none.
pub fn parse_operator(body: &[u8]) -> Result<OperatorTags, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(OperatorTags::new());
    }
    let b: OverlayBody = serde_json::from_slice(body).map_err(|e| format!("body: {e}"))?;
    for (instance, tags) in &b.operator_tags {
        check_labels(None, tags).map_err(|e| format!("operator_tags[{instance:?}]: {e}"))?;
    }
    Ok(b.operator_tags)
}

/// Lays `operator` over the nodes' declared tags. Tags for an instance not in `all` are ignored.
pub fn overlay(all: &mut [Node], operator: &OperatorTags) {
    for n in all {
        if let Some(t) = operator.get(&n.instance) {
            n.tags.extend(t.clone());
            n.operator_tags = t.clone();
        }
    }
}

/// The nodes `filter` selects, instance ascending.
pub fn select(all: Vec<Node>, filter: &Filter) -> Vec<Node> {
    all.into_iter().filter(|n| filter.matches(n)).collect()
}

/// The page `q` asks for out of `all`.
pub fn page(all: Vec<Node>, q: &Query) -> Page {
    let mut selected = select(all, &q.filter);
    let mut rollup = Rollup::default();
    let mut regions: BTreeMap<Option<String>, Rollup> = BTreeMap::new();
    for n in &selected {
        rollup.add(n);
        if q.group_by_region {
            regions.entry(n.region.clone()).or_default().add(n);
        }
    }
    let state_rank = |s: NodeState| match s {
        NodeState::Stale => 0,
        NodeState::Degraded => 1,
        NodeState::Ok => 2,
    };
    selected.sort_by(|a, b| {
        let by_region = if q.group_by_region {
            // `None` sorts last: a regionless node is the odd one out.
            (a.region.is_none(), &a.region).cmp(&(b.region.is_none(), &b.region))
        } else {
            std::cmp::Ordering::Equal
        };
        let by_key = match q.sort {
            SortKey::Instance => a.instance.cmp(&b.instance),
            // Worst first when ascending: the rows an operator opens the page for.
            SortKey::State => state_rank(a.state).cmp(&state_rank(b.state)),
            SortKey::Region => a.region.cmp(&b.region),
            SortKey::Sessions => {
                (a.sessions.tcp + a.sessions.udp).cmp(&(b.sessions.tcp + b.sessions.udp))
            }
            SortKey::Version => a.version.cmp(&b.version),
        };
        let by_key = if q.desc { by_key.reverse() } else { by_key };
        by_region
            .then(by_key)
            .then_with(|| a.instance.cmp(&b.instance))
    });
    let total = selected.len();
    let items = selected
        .into_iter()
        .skip((q.page - 1) * q.size)
        .take(q.size)
        .collect();
    let groups = q.group_by_region.then(|| {
        let mut out: Vec<Group> = regions
            .into_iter()
            .map(|(region, rollup)| Group { region, rollup })
            .collect();
        // `BTreeMap` puts `None` first; the list puts it last.
        out.sort_by(|a, b| (a.region.is_none(), &a.region).cmp(&(b.region.is_none(), &b.region)));
        out
    });
    Page {
        items,
        total,
        page: q.page,
        size: q.size,
        rollup,
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{BackendSummary, IngestPayload, PoolSummary};

    const NOW: u64 = 1_000_000;
    const STALE: u64 = 30_000;

    fn state(
        name: &str,
        region: Option<&str>,
        tags: &[(&str, &str)],
        healthy: bool,
        age_ms: u64,
        sessions: usize,
    ) -> InstanceState {
        InstanceState {
            payload: IngestPayload {
                instance: name.into(),
                admin_url: "http://127.0.0.1:1".into(),
                pools: vec![PoolSummary {
                    name: "p".into(),
                    balancer: "round_robin".into(),
                    backends: vec![BackendSummary {
                        addr: "10.0.0.1:1".into(),
                        healthy,
                        state: "enabled".into(),
                        active: 0,
                    }],
                }],
                sessions: SessionCounts {
                    tcp: sessions,
                    udp: 0,
                },
                group: None,
                version: "0.4.0".into(),
                protocol: "1.2".into(),
                protocol_mismatches: 0,
                region: region.map(String::from),
                tags: tags
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
                listeners: vec![],
            },
            received_at_ms: NOW - age_ms,
            mismatch_rose_at_ms: None,
        }
    }

    /// 7 nodes: eu (a-c, c degraded), us (d, e stale), none (f, g).
    fn fleet() -> Vec<Node> {
        nodes(
            &[
                state("a", Some("eu"), &[("env", "prod")], true, 1_000, 10),
                state(
                    "b",
                    Some("eu"),
                    &[("env", "prod"), ("gpu", "a100")],
                    true,
                    1_000,
                    30,
                ),
                state("c", Some("eu"), &[("env", "stage")], false, 1_000, 20),
                state("d", Some("us"), &[("env", "prod")], true, 1_000, 5),
                state("e", Some("us"), &[], true, 90_000, 0),
                state("f", None, &[], true, 1_000, 1),
                state("g", None, &[("env", "prod")], true, 1_000, 2),
            ],
            NOW,
            STALE,
        )
    }

    fn names(p: &Page) -> Vec<&str> {
        p.items.iter().map(|n| n.instance.as_str()).collect()
    }

    fn q(s: &str) -> Query {
        Query::parse(s).unwrap()
    }

    #[test]
    fn a_node_is_ok_degraded_or_stale() {
        let states: Vec<_> = fleet().into_iter().map(|n| (n.instance, n.state)).collect();
        let want = |n: &str| states.iter().find(|(i, _)| i == n).unwrap().1;
        assert_eq!(want("a"), NodeState::Ok);
        assert_eq!(want("c"), NodeState::Degraded);
        assert_eq!(want("e"), NodeState::Stale);
    }

    #[test]
    fn filters_combine_and_tags_match_by_key_or_key_and_value() {
        let p = page(fleet(), &q("tag=env%3Dprod"));
        assert_eq!(names(&p), ["a", "b", "d", "g"]);
        let p = page(fleet(), &q("tag=env%3Dprod&region=eu"));
        assert_eq!(names(&p), ["a", "b"]);
        let p = page(fleet(), &q("tag=gpu"));
        assert_eq!(names(&p), ["b"]);
        let p = page(fleet(), &q("tag=env%3Dprod&tag=gpu"));
        assert_eq!(names(&p), ["b"], "every tag filter must match");
        let p = page(fleet(), &q("state=degraded"));
        assert_eq!(names(&p), ["c"]);
        let p = page(fleet(), &q("state=stale"));
        assert_eq!(names(&p), ["e"]);
    }

    #[test]
    fn free_text_searches_instance_region_and_tags_ignoring_case() {
        assert_eq!(names(&page(fleet(), &q("q=A100"))), ["b"]);
        assert_eq!(names(&page(fleet(), &q("q=env%3Dstage"))), ["c"]);
        assert_eq!(names(&page(fleet(), &q("q=US"))), ["d", "e"]);
    }

    #[test]
    fn pages_are_sliced_and_total_counts_the_whole_selection() {
        let p = page(fleet(), &q("size=3&page=2"));
        assert_eq!(
            (names(&p), p.total, p.page, p.size),
            (vec!["d", "e", "f"], 7, 2, 3)
        );
        let p = page(fleet(), &q("size=3&page=3"));
        assert_eq!(names(&p), ["g"]);
        let p = page(fleet(), &q("size=3&page=9"));
        assert!(p.items.is_empty());
        assert_eq!(p.total, 7);
    }

    #[test]
    fn rollups_cover_the_selection_not_the_page() {
        let p = page(fleet(), &q("size=2"));
        assert_eq!(
            p.rollup,
            Rollup {
                nodes: 7,
                degraded: 1,
                stale: 1,
                sessions: 68
            }
        );
        let p = page(fleet(), &q("region=eu"));
        assert_eq!((p.rollup.nodes, p.rollup.sessions), (3, 60));
    }

    #[test]
    fn grouping_orders_by_region_with_the_regionless_group_last() {
        let p = page(fleet(), &q("group_by=region&size=100"));
        assert_eq!(names(&p), ["a", "b", "c", "d", "e", "f", "g"]);
        let groups = p.groups.unwrap();
        let shape: Vec<_> = groups
            .iter()
            .map(|g| {
                (
                    g.region.as_deref(),
                    g.rollup.nodes,
                    g.rollup.degraded,
                    g.rollup.stale,
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (Some("eu"), 3, 1, 0),
                (Some("us"), 2, 0, 1),
                (None, 2, 0, 0)
            ]
        );
        assert!(page(fleet(), &q("size=100")).groups.is_none());
    }

    #[test]
    fn sorting_by_state_puts_the_worst_first_and_ties_break_by_instance() {
        let p = page(fleet(), &q("sort=state"));
        assert_eq!(names(&p)[..2], ["e", "c"]);
        let p = page(fleet(), &q("sort=sessions&dir=desc&size=3"));
        assert_eq!(names(&p), ["b", "c", "a"]);
        let p = page(fleet(), &q("sort=instance&dir=desc&size=2"));
        assert_eq!(names(&p), ["g", "f"]);
    }

    #[test]
    fn operator_tags_overlay_the_declared_ones_and_filters_see_the_merge() {
        let mut all = fleet();
        let op = parse_operator(
            br#"{"operator_tags": {"a": {"env": "stage", "rack": "r1"}, "f": {"env": "prod"}, "ghost": {"env": "prod"}}}"#,
        )
        .unwrap();
        overlay(&mut all, &op);
        let a = all.iter().find(|n| n.instance == "a").unwrap();
        assert_eq!(a.tags["env"], "stage", "the operator's value wins");
        assert_eq!(a.tags["rack"], "r1");
        assert_eq!(a.operator_tags.len(), 2, "provenance is kept apart");
        let b = all.iter().find(|n| n.instance == "b").unwrap();
        assert!(b.operator_tags.is_empty() && b.tags["gpu"] == "a100");
        assert_eq!(all.len(), 7, "a tag for an unknown node adds no row");
        assert_eq!(
            names(&page(all.clone(), &q("tag=env%3Dprod"))),
            ["b", "d", "f", "g"]
        );
        assert_eq!(names(&page(all, &q("q=r1"))), ["a"]);
    }

    #[test]
    fn a_bad_operator_overlay_is_refused() {
        assert_eq!(parse_operator(b"").unwrap(), OperatorTags::new());
        for bad in [
            &br#"{"operator_tags": {"a": {"Env": "x"}}}"#[..],
            br#"{"operator_tags": {"a": {"env": ""}}}"#,
            br#"{"operator_tags": {"a": ["env"]}}"#,
            br#"{"operator_tags": 1}"#,
            br#"{"tags": {}}"#,
            b"not json",
        ] {
            assert!(
                parse_operator(bad).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }

    #[test]
    fn bad_queries_are_refused_with_a_reason() {
        for bad in [
            "page=0",
            "size=0",
            "size=201",
            "sort=nope",
            "dir=up",
            "state=sad",
            "group_by=zone",
            "tag=Bad%20Key",
        ] {
            assert!(Query::parse(bad).is_err(), "{bad}");
        }
        assert!(Query::parse(&format!("q={}", "x".repeat(200))).is_err());
        // Unknown keys are ignored so `resolve` takes a list view's query as it is.
        assert!(Filter::parse("page=2&sort=region&region=eu").is_ok());
    }
}

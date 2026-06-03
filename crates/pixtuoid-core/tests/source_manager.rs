use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;

use pixtuoid_core::source::{manager::SourceManager, AgentEvent, Source, TaggedSender, Transport};
use pixtuoid_core::AgentId;

struct StaticSource {
    name: &'static str,
    events: Vec<(Transport, AgentEvent)>,
}

#[async_trait]
impl Source for StaticSource {
    fn name(&self) -> &str {
        self.name
    }
    async fn run(self: Box<Self>, tx: TaggedSender) -> anyhow::Result<()> {
        for ev in self.events {
            tx.send(ev).await?;
        }
        Ok(())
    }
}

/// A source whose `run` returns Err immediately — exercises the manager's
/// log-and-isolate Err arm.
struct FailingSource;

#[async_trait]
impl Source for FailingSource {
    fn name(&self) -> &str {
        "failing"
    }
    async fn run(self: Box<Self>, _tx: TaggedSender) -> anyhow::Result<()> {
        anyhow::bail!("boom")
    }
}

// A source returning Err must be logged and isolated — it must NOT abort its
// siblings. Register a FailingSource beside a good one and assert the good
// source's event still arrives.
#[tokio::test]
async fn manager_isolates_a_failing_source_from_siblings() {
    let good_id = AgentId::from_parts("good", "1");
    let good = StaticSource {
        name: "good",
        events: vec![(
            Transport::Hook,
            AgentEvent::SessionStart {
                agent_id: good_id,
                source: "good".into(),
                session_id: "1".into(),
                cwd: PathBuf::from("/g"),
                parent_id: None,
            },
        )],
    };

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(8);
    let mgr = SourceManager::new()
        .with_source(Box::new(FailingSource))
        .with_source(Box::new(good));
    mgr.spawn(tx);

    let mut got_good = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
        {
            if agent_id == good_id {
                got_good = true;
                break;
            }
        }
    }
    assert!(
        got_good,
        "a failing sibling must be logged-and-isolated, not abort the good source"
    );
}

#[tokio::test]
async fn manager_runs_multiple_sources_concurrently() {
    let id_a = AgentId::from_parts("src-a", "1");
    let id_b = AgentId::from_parts("src-b", "1");

    let src_a = StaticSource {
        name: "src-a",
        events: vec![(
            Transport::Hook,
            AgentEvent::SessionStart {
                agent_id: id_a,
                source: "src-a".into(),
                session_id: "1".into(),
                cwd: PathBuf::from("/a"),
                parent_id: None,
            },
        )],
    };
    let src_b = StaticSource {
        name: "src-b",
        events: vec![(
            Transport::Jsonl,
            AgentEvent::SessionStart {
                agent_id: id_b,
                source: "src-b".into(),
                session_id: "1".into(),
                cwd: PathBuf::from("/b"),
                parent_id: None,
            },
        )],
    };

    let (tx, mut rx) = mpsc::channel::<(Transport, AgentEvent)>(8);
    let mgr = SourceManager::new()
        .with_source(Box::new(src_a))
        .with_source(Box::new(src_b));
    mgr.spawn(tx);

    let mut got_a = false;
    let mut got_b = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some((_, AgentEvent::SessionStart { agent_id, .. }))) =
            tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
        {
            if agent_id == id_a {
                got_a = true;
            }
            if agent_id == id_b {
                got_b = true;
            }
        }
        if got_a && got_b {
            break;
        }
    }
    assert!(got_a, "manager did not deliver event from src-a");
    assert!(got_b, "manager did not deliver event from src-b");
}

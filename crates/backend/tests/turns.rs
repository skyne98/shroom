//! The turn cap is the subagent's: the main agent works until it answers, a
//! capped run stops with a message the user can continue from.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use shroom_ai::{Kind, Message, Role};
use shroom_backend::{Agent, CancellationToken, Update};

/// Answers `rounds` tool calls, then a final answer; counts model requests.
async fn fake(rounds: usize) -> (String, Arc<AtomicUsize>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let counter = seen.clone();
    let url = common::serve(Arc::new(move |_body: &str| {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        if n < rounds {
            common::js("return 1")
        } else {
            common::text("done")
        }
    }))
    .await;
    (url, seen)
}

fn channel() -> tokio::sync::mpsc::UnboundedSender<Update> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    tx
}

#[tokio::test(flavor = "multi_thread")]
async fn the_main_agent_is_not_capped() {
    // More rounds than the old 24: it keeps going until it answers.
    let (url, seen) = fake(30).await;
    let agent = Agent::new(common::model(&url));
    let mut messages = vec![Message::user("go")];
    let run = agent
        .run(&mut messages, &channel(), &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(run.turns, 31);
    assert_eq!(run.message.text(), "done");
    assert_eq!(seen.load(Ordering::SeqCst), 31);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cap_stops_the_run_with_history_intact() {
    let (url, _) = fake(10).await;
    let mut agent = Agent::new(common::model(&url));
    agent.max_turns = Some(2);
    let mut messages = vec![Message::user("go")];
    let error = agent
        .run(&mut messages, &channel(), &CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.kind, Kind::Other);
    assert!(
        error.message.contains("stopped after 2 tool rounds"),
        "{}",
        error.message
    );
    // Every call the model made was answered, so a continue is valid.
    let answered = messages.iter().filter(|m| m.role == Role::Tool).count();
    let calls: usize = messages.iter().map(|m| m.tool_calls().count()).sum();
    assert_eq!(answered, calls);
    assert_eq!(answered, 2);
}

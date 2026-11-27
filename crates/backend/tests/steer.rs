//! Steering: messages queued in an agent's inbox join the run as user turns,
//! and one that arrives as the model is about to finish keeps the run going.

mod common;

use std::sync::{Arc, Mutex};

use shroom_ai::{Message, Role};
use shroom_backend::{Agent, CancellationToken, Inbox, Update};

#[tokio::test(flavor = "multi_thread")]
async fn queued_and_late_messages_steer_the_run() {
    let inbox = Inbox::default();
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let url = {
        let (inbox, bodies) = (inbox.clone(), bodies.clone());
        common::serve(Arc::new(move |body: &str| {
            let mut seen = bodies.lock().unwrap();
            seen.push(body.to_string());
            if seen.len() == 1 {
                // Typed while the first answer streams.
                inbox.lock().unwrap().push("also check the tests".into());
                common::text("first answer")
            } else {
                common::text("answer with tests")
            }
        }))
        .await
    };
    inbox.lock().unwrap().push("use the v2 api".into());
    let agent = Agent::new(common::model(&url)).inbox(inbox.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut messages = vec![Message::user("fix login")];
    let run = agent
        .run(&mut messages, &tx, &CancellationToken::new())
        .await
        .unwrap();
    drop(tx);

    assert_eq!(run.message.text(), "answer with tests");
    assert_eq!(run.turns, 2, "the late message kept the run going");
    let bodies = bodies.lock().unwrap();
    assert!(bodies[0].contains("fix login") && bodies[0].contains("use the v2 api"));
    assert!(bodies[1].contains("also check the tests"));
    // Queued before the start: merged into the first user turn, no two in a row.
    assert_eq!(messages[0].role, Role::User);
    assert!(messages[0].text().contains("use the v2 api"));
    assert_eq!(
        messages.iter().map(|m| m.role).collect::<Vec<_>>(),
        [Role::User, Role::Assistant, Role::User, Role::Assistant]
    );
    let mut steered = Vec::new();
    while let Some(update) = rx.recv().await {
        if let Update::Steered(text) = update {
            steered.push(text);
        }
    }
    assert_eq!(steered, ["use the v2 api", "also check the tests"]);
    assert!(inbox.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_ends_meanwhile_keeps_the_run_going() {
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let url = {
        let bodies = bodies.clone();
        common::serve(Arc::new(move |body: &str| {
            let mut seen = bodies.lock().unwrap();
            seen.push(body.to_string());
            match seen.len() {
                // Start a build that outlives its call, then move on.
                1 => common::js("try { await sh('sleep 1.5; echo built', {timeout: 100}) } catch (e) { return e.message }"),
                2 => {
                    // The model is slow to answer: the job ends meanwhile.
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    common::text("started the build")
                }
                _ => common::text("the build is done"),
            }
        }))
        .await
    };
    let jobs = Arc::new(shroom_js::Jobs::default());
    jobs.notify(|| {});
    let agent = Agent::new(common::model(&url)).jobs(jobs.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut messages = vec![Message::user("build it")];
    let started = std::time::Instant::now();
    let run = agent
        .run(&mut messages, &tx, &CancellationToken::new())
        .await
        .unwrap();
    drop(tx);

    assert_eq!(run.message.text(), "the build is done");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "the program did not wait for its job"
    );
    let bodies = bodies.lock().unwrap();
    assert!(
        bodies[1].contains("You get a message when it ends"),
        "{}",
        bodies[1]
    );
    assert!(
        bodies[2].contains("<job-notification>") && bodies[2].contains("built"),
        "{}",
        bodies[2]
    );
    let mut noted = 0;
    while let Some(update) = rx.recv().await {
        noted += usize::from(matches!(update, Update::Job(_)));
    }
    assert_eq!(noted, 1);
    assert!(!jobs.has_pending());
}

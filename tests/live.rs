//! Every tool against a real postmark2jmap seeded by `evals/seed.sh`:
//!   eval "$(evals/seed.sh)" && cargo test --test live
//! It changes the mailbox (drafts, sends, trash), so re-seed before the next run or the evals.
//! Without JMAP_URL and JMAP_TOKEN it does nothing.

use jmap_mcp::{guard::Guard, jmap::Jmap, tools::*};

async fn tools(allow_write: bool) -> Option<Tools> {
    let (url, token) = (
        std::env::var("JMAP_URL").ok()?,
        std::env::var("JMAP_TOKEN").ok()?,
    );
    Some(Tools {
        jmap: Jmap::connect(&url, &token).await.unwrap(),
        guard: Guard::default(),
        allow_write,
    })
}

fn ids(page: &Page<EmailSummary>) -> Vec<&str> {
    page.items.iter().map(|e| e.id.as_str()).collect()
}

#[tokio::test]
async fn all_tools_against_postmark2jmap() {
    let Some(t) = tools(true).await else {
        eprintln!("skipped: JMAP_URL / JMAP_TOKEN not set");
        return;
    };
    let search = |p: SearchParams| t.search_emails(p);

    // mailboxes
    let mb = t.list_mailboxes(None, None).await.unwrap();
    let role = |r: &str| {
        mb.items
            .iter()
            .find(|m| m.role.as_deref() == Some(r))
            .unwrap()
            .id
            .clone()
    };
    assert!(mb.total >= 5);
    assert_eq!(
        mb.items
            .iter()
            .find(|m| m.role.as_deref() == Some("inbox"))
            .unwrap()
            .total_emails,
        11
    );

    // search, filters, pagination
    let inv = search(SearchParams {
        text: Some("invoice".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(inv.total, 2);
    let p1 = search(SearchParams {
        limit: Some(4),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(
        (p1.total, p1.count, p1.has_more, p1.next_offset),
        (11, 4, true, Some(4))
    );
    let p3 = search(SearchParams {
        limit: Some(4),
        offset: Some(8),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!((p3.count, p3.has_more), (3, false));
    assert!(
        p1.items[0].received_at > p1.items[1].received_at,
        "newest first"
    );
    let unread = search(SearchParams {
        unread_only: Some(true),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(unread.total, 7);
    let from = search(SearchParams {
        from: Some("carol".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(from.total, 1);
    let after = search(SearchParams {
        after: Some("2026-06-09T00:00:00Z".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(after.total, 3); // e5 (06-11), e10 and e11 (06-09)

    // get_email / get_thread
    let orion = &from.items[0];
    let e = t.get_email(&orion.id, None).await.unwrap();
    assert!(e.body.contains("$85k") && !e.body_truncated);
    assert_eq!(e.cc[0].email, "dave@example.com");
    let cut = t.get_email(&orion.id, Some(5)).await.unwrap();
    assert!(cut.body_truncated && cut.body.chars().count() == 5);
    let th = t.get_thread(&orion.thread_id, None, None).await.unwrap();
    assert_eq!(th.total, 2);
    assert!(t.get_email("nope", None).await.is_err());

    // mark / move / trash
    let news = search(SearchParams {
        from: Some("news@weekly".into()),
        ..Default::default()
    })
    .await
    .unwrap()
    .items
    .remove(0);
    assert!(news.unread);
    let r = t
        .mark_emails(std::slice::from_ref(&news.id), MarkAction::Read)
        .await
        .unwrap();
    assert_eq!((r.updated.len(), r.failed.len()), (1, 0));
    assert_eq!(
        search(SearchParams {
            unread_only: Some(true),
            ..Default::default()
        })
        .await
        .unwrap()
        .total,
        6
    );
    let r = t
        .mark_emails(&[news.id.clone(), "nope".into()], MarkAction::Flagged)
        .await
        .unwrap();
    assert_eq!((r.updated.len(), r.failed.len()), (1, 1));
    t.move_emails(std::slice::from_ref(&news.id), &role("junk"))
        .await
        .unwrap();
    let moved = search(SearchParams {
        mailbox_id: Some(role("junk")),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&moved), [news.id.as_str()]);
    t.trash_emails(std::slice::from_ref(&news.id))
        .await
        .unwrap();
    assert_eq!(
        search(SearchParams {
            mailbox_id: Some(role("trash")),
            ..Default::default()
        })
        .await
        .unwrap()
        .total,
        1
    );

    // drafts: create, update (new id), delete, refuse non-drafts
    let d = t
        .create_draft(DraftParams {
            to: vec![Addr {
                email: "alice@example.com".into(),
                name: Some("Alice".into()),
            }],
            subject: "Hello".into(),
            body: "first".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    let d2 = t
        .update_draft(DraftUpdate {
            id: d.id.clone(),
            body: Some("second".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_ne!(d.id, d2.id);
    assert_eq!(d2.replaced_id.as_deref(), Some(d.id.as_str()));
    assert!(t.get_email(&d.id, None).await.is_err(), "old draft is gone");
    let got = t.get_email(&d2.id, None).await.unwrap();
    assert_eq!(
        (got.body.trim(), got.summary.subject.as_str()),
        ("second", "Hello")
    );
    assert!(t.delete_draft(&orion.id).await.is_err(), "not a draft");
    t.delete_draft(&d2.id).await.unwrap();
    assert!(
        t.create_draft(DraftParams {
            to: vec![],
            subject: "x".into(),
            body: "x".into(),
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert!(
        t.create_draft(DraftParams {
            to: vec![Addr {
                email: "a@b".into(),
                name: None
            }],
            subject: "x\r\nBcc: z".into(),
            body: "x".into(),
            ..Default::default()
        })
        .await
        .is_err()
    );

    // reply stages by default, reply-all keeps Cc out of To, then send_draft once
    let rep = t
        .reply_to_message(ReplyParams {
            message_id: orion.id.clone(),
            body: "Thanks".into(),
            reply_all: Some(true),
            send: None,
            identity_id: None,
        })
        .await
        .unwrap();
    assert!(rep.sent.is_none());
    assert_eq!(
        (rep.subject.as_str(), rep.to[0].email.as_str()),
        ("Re: Project Orion kickoff", "carol@example.com")
    );
    let sent = t.send_draft(&rep.draft_id, None).await.unwrap();
    assert_eq!(
        (
            sent.undo_status.as_str(),
            sent.replayed,
            sent.warning.is_none()
        ),
        ("pending", false, true)
    );
    assert_eq!(
        sent.delivery[0].status, "queued",
        "no Postmark: accepted, not delivered"
    );
    let again = t.send_draft(&rep.draft_id, None).await.unwrap();
    assert!(again.replayed && again.submission_id == sent.submission_id);
    let in_sent = search(SearchParams {
        mailbox_id: Some(role("sent")),
        ..Default::default()
    })
    .await
    .unwrap();
    assert_eq!(ids(&in_sent), [rep.draft_id.as_str()]);
    assert!(
        t.get_email(&rep.draft_id, None)
            .await
            .unwrap()
            .summary
            .keywords
            .iter()
            .all(|k| k != "$draft")
    );
    // a second guard (a restarted server) still cannot send it: it is no longer a draft
    let fresh = tools(true).await.unwrap();
    assert!(
        fresh
            .send_draft(&rep.draft_id, None)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a draft")
    );

    // reply with send=true, retried: one submission
    let lunch = search(SearchParams {
        subject: Some("Lunch Friday".into()),
        ..Default::default()
    })
    .await
    .unwrap()
    .items
    .remove(0);
    let rp = ReplyParams {
        message_id: lunch.id.clone(),
        body: "See you there".into(),
        reply_all: None,
        send: Some(true),
        identity_id: None,
    };
    let a = t
        .reply_to_message(ReplyParams { ..clone(&rp) })
        .await
        .unwrap();
    let b = t.reply_to_message(clone(&rp)).await.unwrap();
    assert!(a.sent.is_some() && b.replayed && b.draft_id == a.draft_id);
    assert_eq!(
        search(SearchParams {
            mailbox_id: Some(role("sent")),
            ..Default::default()
        })
        .await
        .unwrap()
        .total,
        2
    );

    // read-only mode refuses every write
    let ro = tools(false).await.unwrap();
    assert!(
        ro.move_emails(std::slice::from_ref(&news.id), &role("inbox"))
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    assert!(
        ro.send_draft("x", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("read-only")
    );
    assert!(ro.list_mailboxes(None, None).await.is_ok());
}

fn clone(p: &ReplyParams) -> ReplyParams {
    ReplyParams {
        message_id: p.message_id.clone(),
        body: p.body.clone(),
        reply_all: p.reply_all,
        send: p.send,
        identity_id: p.identity_id.clone(),
    }
}

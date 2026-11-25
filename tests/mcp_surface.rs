//! What an MCP client sees: which tools are listed in each mode, and that every tool
//! declares input and output schemas. Uses the fake JMAP server from the guard tests' shape.

use axum::{Json, Router, routing::get};
use jmap_mcp::{guard::Guard, jmap::Jmap, server::Server, tools::Tools};
use rmcp::ServerHandler;
use serde_json::json;

async fn server(allow_write: bool) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let doc = json!({
        "capabilities": {"urn:ietf:params:jmap:core": {}, "urn:ietf:params:jmap:mail": {}, "urn:ietf:params:jmap:submission": {}},
        "primaryAccounts": {"urn:ietf:params:jmap:mail": "a1"}, "apiUrl": format!("{base}/jmap")
    });
    let app = Router::new().route("/.well-known/jmap", get(move || async move { Json(doc) }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let jmap = Jmap::connect(&base, "t").await.unwrap();
    Server::new(Tools {
        jmap,
        guard: Guard::default(),
        allow_write,
    })
}

#[tokio::test]
async fn read_only_lists_four_tools_and_write_lists_twelve() {
    let ro: Vec<String> = server(false)
        .await
        .tool_list()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect();
    assert_eq!(
        ro,
        [
            "jmap_get_email",
            "jmap_get_thread",
            "jmap_list_mailboxes",
            "jmap_search_emails"
        ]
    );
    let rw = server(true).await.tool_list();
    assert_eq!(rw.len(), 12);
    for t in &rw {
        assert!(t.output_schema.is_some(), "{} has no output schema", t.name);
        assert!(t.annotations.is_some(), "{} has no annotations", t.name);
        assert!(t.name.starts_with("jmap_"));
    }
    // Unsigned integer formats (`uint64`) make clients warn about the schema.
    let schemas = serde_json::to_string(&rw).unwrap();
    assert!(!schemas.contains("\"format\":\"uint"), "{schemas}");
    let _ = ServerHandler::get_info(&server(false).await);
}

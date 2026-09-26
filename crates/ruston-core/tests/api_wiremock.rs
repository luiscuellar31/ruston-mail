//! Wire-level tests for the API layer against a mock Proton server.

use ruston_core::api;
use ruston_core::api::messages::ListQuery;
use ruston_core::model::enums::resolve_folder;
use ruston_core::transport::HttpClient;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(uri: &str) -> HttpClient {
    HttpClient::new(uri, "Other")
}

#[tokio::test]
async fn list_messages_builds_query_and_decodes() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mail/v4/messages"))
        .and(query_param("Sort", "Time"))
        .and(query_param("Desc", "1"))
        .and(query_param("LabelID", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "Total": 2,
            "Messages": [
                {"ID": "m1", "Subject": "hi", "Sender": {"Address": "a@proton.me"}},
                {"ID": "m2", "Subject": "yo", "Sender": {"Address": "b@x.com"}}
            ]
        })))
        .mount(&server)
        .await;

    let q = ListQuery {
        label_id: Some("0".into()),
        ..Default::default()
    };
    let (total, msgs) = api::messages::list_messages(&client(&server.uri()), &q)
        .await
        .unwrap();
    assert_eq!(total, 2);
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0].id, "m1");
}

#[tokio::test]
async fn get_conversation_decodes_message_metadata() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mail/v4/conversations/c1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "Conversation": {"ID": "c1", "NumMessages": 2},
            "Messages": [
                {
                    "ID": "m2", "ConversationID": "c1", "Time": 20, "Flags": 32,
                    "Sender": {"Address": "a@example.com"}, "LabelIDs": ["0"]
                },
                {
                    "ID": "m1", "ConversationID": "c1", "Time": 10,
                    "Sender": {"Address": "a@example.com"}, "LabelIDs": ["0", "10"]
                }
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let (conversation, messages) =
        api::conversations::get_conversation(&client(&server.uri()), "c1")
            .await
            .unwrap();
    assert_eq!(conversation.id, "c1");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].meta.id, "m2");
    assert_eq!(messages[0].meta.flags, 32);
    assert_eq!(messages[1].meta.label_ids, ["0", "10"]);
    assert!(messages[0].body.is_empty());
}

#[tokio::test]
async fn trash_posts_label_3() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/mail/v4/messages/label"))
        .and(body_json(
            serde_json::json!({ "LabelID": "3", "IDs": ["x"] }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})))
        .expect(1)
        .mount(&server)
        .await;
    api::messages::label(&client(&server.uri()), "3", &["x".to_string()])
        .await
        .unwrap();
}

#[tokio::test]
async fn labels_list_builds_type_param() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/core/v4/labels"))
        .and(query_param("Type", "3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "Labels": [{"ID": "f1", "Name": "Work", "Type": 3, "Color": "#fff", "Path": "Work"}]
        })))
        .mount(&server)
        .await;
    let folders = api::labels::list(&client(&server.uri()), 3).await.unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].name, "Work");
}

#[tokio::test]
async fn recipient_keys_lookup() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/core/v4/keys/all"))
        .and(query_param("Email", "bob@proton.me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "RecipientType": 1,
            "Address": { "Keys": [{"PublicKey": "ARMORED", "Flags": 3, "Primary": 1}] }
        })))
        .mount(&server)
        .await;
    let rk = api::keys::get_all_public_keys(&client(&server.uri()), "bob@proton.me")
        .await
        .unwrap();
    assert_eq!(rk.recipient_type, 1);
    assert_eq!(rk.keys.len(), 1);
    assert_eq!(rk.keys[0].flags, 3);
}

#[tokio::test]
async fn a_label_id_reaches_the_wire_as_it_was_given() {
    // A folder or label the account made is addressed by its own ID, which is
    // case-sensitive. The mock only answers the exact one, so a query that
    // arrives folded to lower case gets no match and the call fails.
    let id = "qBIcv1_Wv5X4hLpEo0Tz9A==";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mail/v4/conversations"))
        .and(query_param("LabelID", id))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1000,
            "Total": 0,
            "Conversations": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    // The query Client::list_conversations builds for that folder.
    let q = ListQuery {
        label_id: Some(resolve_folder(id)),
        page: Some(0),
        page_size: Some(50),
        ..Default::default()
    };
    let (total, conversations) = api::conversations::list_conversations(&client(&server.uri()), &q)
        .await
        .expect("the label was addressed as Proton named it");

    assert_eq!(total, 0);
    assert!(conversations.is_empty());
}

//! The API functions, run against a mock of Proton's server: what each call
//! sends, and how it reads what comes back.

use ruston_core::api;
use ruston_core::api::messages::ListQuery;
use ruston_core::model::enums::{label_ids, label_type, recipient_type, resolve_folder};
use ruston_core::transport::HttpClient;
use wiremock::matchers::{body_json, method, path, query_param};
use wiremock::{Mock, MockBuilder, MockServer, ResponseTemplate};

fn client(uri: &str) -> HttpClient {
    HttpClient::new(uri, "Other")
}

/// Matches a `verb` request to `endpoint` that carries each name and value
/// of `query`.
fn request(verb: &str, endpoint: &str, query: &[(&str, &str)]) -> MockBuilder {
    query.iter().fold(
        Mock::given(method(verb)).and(path(endpoint)),
        |mock, (name, value)| mock.and(query_param(*name, *value)),
    )
}

/// Proton's answer to a request that succeeded: code 1000 and `fields`.
fn success(mut fields: serde_json::Value) -> ResponseTemplate {
    fields["Code"] = 1000.into();
    ResponseTemplate::new(200).set_body_json(fields)
}

#[tokio::test]
async fn a_folder_page_is_asked_for_newest_first_and_read_with_its_total() {
    let server = MockServer::start().await;
    let newest_first = [("Sort", "Time"), ("Desc", "1"), ("LabelID", "0")];
    request("GET", "/mail/v4/messages", &newest_first)
        .respond_with(success(serde_json::json!({
            "Total": 2,
            "Messages": [
                {"ID": "msg-new", "Subject": "Invoice", "Sender": {"Address": "billing@example.org"}},
                {"ID": "msg-old", "Subject": "Welcome", "Sender": {"Address": "team@proton.me"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let inbox = ListQuery {
        label_id: Some(label_ids::INBOX.into()),
        ..ListQuery::default()
    };
    let (total, messages) = api::messages::list_messages(&client(&server.uri()), &inbox)
        .await
        .unwrap();
    assert_eq!(total, 2);
    let ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["msg-new", "msg-old"]);
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
async fn a_bulk_action_names_the_ids_proton_refused() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/mail/v4/messages/label"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1001,
            "Responses": [
                {"ID": "a", "Response": {"Code": 1000}},
                {"ID": "b", "Response": {"Code": 2501, "Error": "Message does not exist"}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/mail/v4/conversations/label"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "Code": 1001,
            "Responses": [
                {"ID": "c1", "Response": {"Code": 1000}},
                {"ID": "c2", "Response": {"Code": 1000}}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let http = client(&server.uri());

    let ids = ["a".to_string(), "b".to_string()];
    let error = api::messages::label(&http, "6", &ids).await.unwrap_err();
    let ruston_core::Error::Api(refused) = error else {
        panic!("expected an API error, got {error:?}");
    };
    assert_eq!(refused.code, 2501);
    assert!(
        refused.message.contains("1 of 2")
            && refused.message.contains("b (Message does not exist)"),
        "{}",
        refused.message
    );
    // Every ID taken is a success.
    api::conversations::label(&http, "6", &["c1".to_string(), "c2".to_string()])
        .await
        .unwrap();
}

#[tokio::test]
async fn trashing_puts_the_trash_label_on_the_messages() {
    let server = MockServer::start().await;
    request("PUT", "/mail/v4/messages/label", &[])
        .and(body_json(
            serde_json::json!({ "LabelID": "3", "IDs": ["msg-1"] }),
        ))
        .respond_with(success(serde_json::json!({})))
        .expect(1)
        .mount(&server)
        .await;
    let ids = ["msg-1".to_string()];
    api::messages::label(&client(&server.uri()), label_ids::TRASH, &ids)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_conversation_is_labelled_and_unlabelled_as_a_whole() {
    let server = MockServer::start().await;
    for op in ["label", "unlabel"] {
        Mock::given(method("PUT"))
            .and(path(format!("/mail/v4/conversations/{op}")))
            .and(body_json(
                serde_json::json!({ "LabelID": "wN2", "IDs": ["c1"] }),
            ))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"Code": 1000})),
            )
            .expect(1)
            .mount(&server)
            .await;
    }
    let http = client(&server.uri());
    let ids = ["c1".to_string()];
    api::conversations::label(&http, "wN2", &ids).await.unwrap();
    api::conversations::unlabel(&http, "wN2", &ids)
        .await
        .unwrap();
}

#[tokio::test]
async fn folders_are_listed_by_their_label_type() {
    let server = MockServer::start().await;
    request("GET", "/core/v4/labels", &[("Type", "3")])
        .respond_with(success(serde_json::json!({
            "Labels": [
                {"ID": "folder-1", "Name": "Receipts", "Type": 3, "Color": "#8080FF", "Path": "Receipts"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let folders = api::labels::list(&client(&server.uri()), label_type::MESSAGE_FOLDER)
        .await
        .unwrap();
    let [folder] = folders.as_slice() else {
        panic!("one folder, got {folders:?}");
    };
    assert_eq!(folder.name, "Receipts");
}

#[tokio::test]
async fn a_recipient_s_keys_are_looked_up_by_address() {
    let server = MockServer::start().await;
    request("GET", "/core/v4/keys/all", &[("Email", "bo@proton.me")])
        .respond_with(success(serde_json::json!({
            "RecipientType": 1,
            "Address": {
                "Keys": [{"PublicKey": "-----BEGIN PGP PUBLIC KEY BLOCK-----", "Flags": 3, "Primary": 1}]
            }
        })))
        .expect(1)
        .mount(&server)
        .await;
    let found = api::keys::get_all_public_keys(&client(&server.uri()), "bo@proton.me")
        .await
        .unwrap();
    assert_eq!(found.recipient_type, recipient_type::INTERNAL);
    let [key] = found.keys.as_slice() else {
        panic!("one key, got {:?}", found.keys);
    };
    assert_eq!(key.flags, 3);
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

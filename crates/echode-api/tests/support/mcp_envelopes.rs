//! Invalid response envelopes shared by the pre-edit and post-edit socket tests.

use serde_json::{Value, json};

pub fn invalid_envelopes(id: u32, result: Value) -> Vec<Value> {
    let valid = json!({"jsonrpc":"2.0", "id":id, "result":result});
    let mut responses = vec![
        Value::Null,
        json!(false),
        json!(7),
        json!("not an envelope"),
        json!([]),
        json!([valid.clone()]),
        json!({}),
    ];
    for field in ["jsonrpc", "id"] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(field);
        responses.push(missing);
    }
    for version in [
        Value::Null,
        json!(false),
        json!(2),
        json!(2.0),
        json!([]),
        json!({}),
        json!("1.0"),
        json!(""),
    ] {
        let mut response = valid.clone();
        response["jsonrpc"] = version;
        responses.push(response);
    }
    for wrong_id in [
        Value::Null,
        json!(false),
        json!(true),
        json!(id + 1),
        json!(id.to_string()),
        json!(f64::from(id)),
        json!(i64::MIN),
        json!(u64::MAX),
        json!([]),
        json!({}),
    ] {
        let mut response = valid.clone();
        response["id"] = wrong_id;
        responses.push(response);
    }
    let mut both = valid.clone();
    // Keep this error valid so only the result/error exclusivity check rejects it.
    both["error"] = json!({"code":-32000, "message":"contradictory outcome"});
    responses.push(both);
    let mut neither = valid;
    neither.as_object_mut().unwrap().remove("result");
    responses.push(neither);
    responses
}

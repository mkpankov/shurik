use ::hyper::Client;
use ::hyper::header::{ContentType};
use ::hyper::mime::{Mime, TopLevel, SubLevel};
use ::iron::*;

pub fn post_comment(api_root: &str,
                    private_token: &str,
                    mr_id: u64,
                    target_project_id: u64,
                    message: &str) {
    let client = Client::new();
    let mut headers = Headers::new();
    headers.set(ContentType(Mime(TopLevel::Application, SubLevel::Json, vec![])));
    headers.set_raw("PRIVATE-TOKEN", vec![private_token.to_owned().into_bytes()]);

    debug!("headers == {:?}", headers);

    info!("Posting comment: {}", message);
    let res = client.post(&*format!("{}/projects/{}/merge_request/{}/comments", api_root, target_project_id, mr_id))
        .headers(headers)
        .body(message)
        .send()
        .unwrap();
    assert_eq!(res.status, ::hyper::status::StatusCode::Created);
}

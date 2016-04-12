use ::std::io::Read;
use ::std::path::PathBuf;

use ::hyper::Client;
use ::hyper::client::IntoUrl;
use ::hyper::header::{ContentType};
use ::hyper::mime::{Mime, TopLevel, SubLevel};
use ::hyper::Url;
use ::iron::{Headers};

use ::MrUid;

pub struct Api {
    root: Url,
    key_path: PathBuf,
}

#[derive(Debug)]
pub struct JsonError;
#[derive(Debug)]
pub struct JsonObjectError;
#[derive(Debug)]
pub struct JsonObjectStringError;

quick_error! {
    #[derive(Debug)]
    pub enum LoginError {
        /// IO Error
        Http(err: ::hyper::error::Error) {
            from()
        }
        /// Utf8 Error
        Read(err: ::std::io::Error) {
            from()
        }
        Json(err: JsonError) {
            from()
        }
        JsonObject(err: JsonObjectError) {
            from()
        }
        JsonObjectString(err: JsonObjectStringError) {
            from()
        }
    }
}

impl Api {
    pub fn new<T: IntoUrl>(maybe_url: T, key_path: &str) -> Result<Self, ::url::ParseError> {
        let url = try!(maybe_url.into_url());
        let mut path = PathBuf::new();
        path.push(key_path);
        Ok(
            Api {
                root: url,
                key_path: path,
            })
    }

    pub fn login(&self, username: &str, password: &str) -> Result<Session, LoginError> {
        let client = Client::new();

        let mut res =
            try!(client.post(&*format!("{}/session", self.root))
                 .body(&*format!("login={}&password={}", username, password))
                 .send());
        assert_eq!(res.status, ::hyper::status::StatusCode::Created);
        let mut text = String::new();
        try!(res.read_to_string(&mut text));
        let json: ::serde_json::value::Value = ::serde_json::from_str(&text).unwrap();
        debug!("object? {}", json.is_object());
        let obj = try!(json.as_object().ok_or(JsonError));
        let private_token_value = try!(obj.get("private_token").ok_or(JsonObjectError));
        let private_token = try!(private_token_value.as_string().ok_or(JsonObjectStringError));
        info!("Logged in to GitLab");
        debug!("private_token == {}", private_token);

        Ok(
            Session {
                root: self.root.clone(),
                key_path: self.key_path.clone(),
                private_token: private_token.to_owned(),
            })
    }
}

pub struct Session {
    root: Url,
    key_path: PathBuf,
    private_token: String,
}

impl Session {
    pub fn post_comment(&self,
                        mr_id: MrUid,
                        message: &str) {
        let api_root = &self.root;
        let private_token = &self.private_token;

        let client = Client::new();
        let mut headers = Headers::new();
        headers.set(ContentType(Mime(TopLevel::Application, SubLevel::Json, vec![])));
        headers.set_raw("PRIVATE-TOKEN", vec![private_token.to_owned().into_bytes()]);

        let MrUid { target_project_id, id } = mr_id;

        info!("Posting comment: {}", message);
        let url = &*format!("{}/projects/{}/merge_request/{}/comments", api_root, target_project_id, id);
        debug!("url == {:?}", url);
        debug!("headers == {:?}", headers);
        debug!("body == {:?}", message);
        let res = client.post(url)
            .headers(headers)
            .body(message)
            .send()
            .unwrap();
        assert_eq!(res.status, ::hyper::status::StatusCode::Created);
    }

}

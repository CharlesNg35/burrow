use burrow_admin_proto::msg::{self, ErrorCode, HistoryRequest, Suggest};
use serde_json::Value;

use super::{ApiError, Core, Reply, Session, passwords, plan, snippets, uploads};

pub fn ok<T: serde::Serialize>(v: T) -> Reply {
    serde_json::to_value(v).map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))
}

pub fn parse<T: serde::de::DeserializeOwned>(body: &Value) -> Result<T, ApiError> {
    let body = if body.is_null() {
        Value::Object(Default::default())
    } else {
        body.clone()
    };
    serde_json::from_value(body)
        .map_err(|e| ApiError::new(ErrorCode::BadRequest, format!("bad request body: {e}")))
}

pub fn dispatch(core: &Core, s: &Session, t: &str, body: &Value) -> Reply {
    match t {
        msg::req::WELCOME => {
            let r: msg::WelcomeRequest = parse(body).unwrap_or_default();
            s.hello(r.burrow, r.understands);
            ok(core.welcome(s))
        }
        msg::req::SCHEMA => {
            let r: msg::SchemaRequest = parse(body)?;
            ok(core.schema_reply(&r.have))
        }
        msg::req::STATE => ok(core.state(s)?),
        msg::req::VALIDATE => {
            let r: msg::Validate = parse(body)?;
            if !s.writes() {
                return Err(ApiError::new(
                    ErrorCode::Forbidden,
                    "read access cannot validate changes",
                ));
            }
            ok(plan::compute(core, s, &r.base, &r.ops)?.plan)
        }
        msg::req::GENERATE_SECRET => ok(core.generate_secret(s)?),
        msg::req::SUGGEST => {
            let r: Suggest = parse(body)?;
            ok(core.suggest(&r)?)
        }
        msg::req::HISTORY => {
            let r: HistoryRequest = parse(body)?;
            ok(core.history(&r)?)
        }
        msg::req::REVERT => {
            let r: msg::Revert = parse(body)?;
            if !s.writes() {
                return Err(ApiError::new(
                    ErrorCode::Forbidden,
                    "read access cannot revert",
                ));
            }
            let ops = [burrow_admin_proto::ops::Op::ReplaceDocument { from_history: r.to }];
            ok(plan::compute(core, s, &r.base, &ops)?.plan)
        }
        msg::req::CONNECT_INFO => {
            let r: msg::ConnectInfo = parse(body)?;
            ok(snippets::connect_info(core, &r)?)
        }
        msg::req::PING => ok(serde_json::json!({})),
        msg::req::UPLOAD => {
            let r: msg::Upload = parse(body)?;
            ok(uploads::upload(core, s, &r)?)
        }
        msg::req::LOCAL_CA => uploads::local_ca(core),
        msg::req::HASH_PASSWORD => {
            let r: msg::HashPassword = parse(body)?;
            ok(passwords::hash_password(s, r)?)
        }
        other => Err(ApiError::new(
            ErrorCode::UnknownRequest,
            format!("unknown request \"{other}\""),
        )),
    }
}

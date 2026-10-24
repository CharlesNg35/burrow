use std::net::IpAddr;
use std::time::Duration;

use instant_acme::{
    Account, AuthorizationStatus, CertificateIdentifier, ChallengeType, Identifier, NewOrder,
    OrderStatus, RetryPolicy,
};

use super::challenge::Challenges;
use super::{Problemish, describe, problem_text};
use crate::tls::CertCheck;

pub const POLL_TIMEOUT: Duration = Duration::from_secs(180);

pub struct Ask<'a> {
    pub names: &'a [String],
    pub check: CertCheck,
    pub profile: Option<&'a str>,
    pub replaces: Option<CertificateIdentifier<'static>>,
}

#[derive(Debug, Clone)]
pub struct Issued {
    pub chain_pem: String,
    pub key_pem: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failed {
    pub text: String,
    pub kind: Problemish,
    pub validation: bool,
}

impl Failed {
    fn of(e: &instant_acme::Error) -> Failed {
        Failed {
            text: describe(e),
            kind: Problemish::of(e),
            validation: false,
        }
    }

    fn validation(text: String) -> Failed {
        Failed {
            text,
            kind: Problemish::Other,
            validation: true,
        }
    }
}

pub fn identifier(name: &str) -> Identifier {
    match name.parse::<IpAddr>() {
        Ok(ip) => Identifier::Ip(ip),
        Err(_) => Identifier::Dns(name.to_string()),
    }
}

pub async fn issue(
    account: &Account,
    challenges: &Challenges,
    ask: Ask<'_>,
) -> Result<Issued, Failed> {
    let ids: Vec<Identifier> = ask.names.iter().map(|n| identifier(n)).collect();
    let mut new = NewOrder::new(&ids);
    if let Some(p) = ask.profile {
        new = new.profile(p);
    }
    if let Some(r) = ask.replaces {
        new = new.replaces(r);
    }
    let mut order = account.new_order(&new).await.map_err(|e| Failed::of(&e))?;
    let mut tokens = Vec::new();
    let answered = answer(&mut order, challenges, ask.check, &mut tokens).await;
    let done = match answered {
        Ok(()) => finish(&mut order).await,
        Err(f) => Err(f),
    };
    challenges.forget(ask.names, &tokens);
    done
}

async fn answer(
    order: &mut instant_acme::Order,
    challenges: &Challenges,
    check: CertCheck,
    tokens: &mut Vec<String>,
) -> Result<(), Failed> {
    let kind = match check {
        CertCheck::Tls => ChallengeType::TlsAlpn01,
        CertCheck::Http => ChallengeType::Http01,
    };
    let mut auths = order.authorizations();
    while let Some(a) = auths.next().await {
        let mut a = a.map_err(|e| Failed::of(&e))?;
        match a.status {
            AuthorizationStatus::Valid => continue,
            AuthorizationStatus::Pending => {}
            other => {
                return Err(Failed::validation(format!(
                    "the check of {} is {other:?}",
                    a.identifier()
                )));
            }
        }
        let name = a.identifier().to_string();
        let mut ch = a.challenge(kind.clone()).ok_or_else(|| {
            Failed::validation(format!("the CA offers no {kind:?} check for {name}"))
        })?;
        let ka = ch.key_authorization();
        match check {
            CertCheck::Tls => challenges.answer_alpn(&name, ka.digest().as_ref()),
            CertCheck::Http => {
                tokens.push(ch.token.clone());
                challenges.answer_http(&ch.token, ka.as_str())
            }
        }
        .map_err(Failed::validation)?;
        ch.set_ready().await.map_err(|e| Failed::of(&e))?;
    }
    Ok(())
}

async fn finish(order: &mut instant_acme::Order) -> Result<Issued, Failed> {
    let policy = RetryPolicy::new().timeout(POLL_TIMEOUT);
    let status = match order.poll_ready(&policy).await {
        Ok(s) => s,
        Err(instant_acme::Error::Api(p)) => {
            let why = check_errors(order).await;
            return Err(Failed::validation(why.unwrap_or_else(|| problem_text(&p))));
        }
        Err(e) => return Err(Failed::of(&e)),
    };
    if status != OrderStatus::Ready {
        let why = check_errors(order).await;
        return Err(Failed::validation(
            why.unwrap_or_else(|| "the CA did not accept the checks".into()),
        ));
    }
    let key_pem = order.finalize().await.map_err(|e| Failed::of(&e))?;
    let chain_pem = order
        .poll_certificate(&policy)
        .await
        .map_err(|e| Failed::of(&e))?;
    Ok(Issued { chain_pem, key_pem })
}

async fn check_errors(order: &mut instant_acme::Order) -> Option<String> {
    let mut out = Vec::new();
    let mut auths = order.authorizations();
    while let Some(Ok(a)) = auths.next().await {
        for c in &a.challenges {
            if let Some(p) = &c.error {
                out.push(format!("{}: {}", a.identifier(), problem_text(p)));
            }
        }
    }
    (!out.is_empty()).then(|| out.join("; "))
}

use hickory_proto::op::{Message, MessageType, OpCode};
use hickory_proto::rr::RecordType;

use super::super::fake::{Fake, txt};
use super::*;

#[tokio::test]
async fn a_question_gets_its_answer_over_udp_and_tcp() {
    let f = Fake::start(true).await;
    f.add(txt("_acme-challenge.example.com", "abc"));
    let q = question(
        &name("_acme-challenge.example.com").unwrap(),
        RecordType::TXT,
        false,
    );
    let m = ask(f.addr, &q).await.unwrap();
    assert!(m.metadata.authoritative);
    assert_eq!(m.answers.len(), 1);
    let raw = tcp(f.addr, &q.to_vec().unwrap(), QUERY_TIMEOUT)
        .await
        .unwrap();
    assert_eq!(reply_to(&q, &raw).unwrap().answers.len(), 1);
}

#[test]
fn an_answer_to_another_question_or_id_is_refused() {
    let n = name("a.example.com").unwrap();
    let q = question(&n, RecordType::TXT, true);
    let mut wrong_id = Message::new(
        q.metadata.id.wrapping_add(1),
        MessageType::Response,
        OpCode::Query,
    );
    wrong_id.add_query(q.queries[0].clone());
    assert!(reply_to(&q, &wrong_id.to_vec().unwrap()).is_err());
    let mut other = Message::new(q.metadata.id, MessageType::Response, OpCode::Query);
    other.add_query(hickory_proto::op::Query::query(
        name("b.example.com").unwrap(),
        RecordType::TXT,
    ));
    assert!(reply_to(&q, &other.to_vec().unwrap()).is_err());
    let mut not_reply = Message::new(q.metadata.id, MessageType::Query, OpCode::Query);
    not_reply.add_query(q.queries[0].clone());
    assert!(reply_to(&q, &not_reply.to_vec().unwrap()).is_err());
    let mut good = Message::new(q.metadata.id, MessageType::Response, OpCode::Query);
    good.add_query(q.queries[0].clone());
    assert!(reply_to(&q, &good.to_vec().unwrap()).is_ok());
}

#[test]
fn names_are_absolute_and_shown_without_the_final_dot() {
    let n = name("Wiki.Example.com").unwrap();
    assert!(n.is_fqdn());
    assert_eq!(shown(&n), "wiki.example.com");
    let reach = Reach::default();
    assert_eq!(
        reach.nameserver("192.0.2.1".parse().unwrap()),
        "192.0.2.1:53".parse().unwrap()
    );
}

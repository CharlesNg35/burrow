use burrow_admin_proto::schema::Entity;

#[derive(Debug, PartialEq, Eq)]
pub struct Info {
    pub singular: &'static str,
    pub icon: &'static str,
    pub name_example: &'static str,
    pub intro: &'static str,
    pub rename_note: &'static str,
}

const CLIENT: Info = Info {
    singular: <burrow_config::ClientEntry as burrow_config::Entry>::ENTITY,
    icon: "monitor",
    name_example: "home or office",
    intro: "A machine that dials out to this server and reaches targets for your services.",
    rename_note: "The client.yaml on that machine must use the new name, or the client cannot connect.",
};

const VISITOR: Info = Info {
    singular: <burrow_config::VisitorEntry as burrow_config::Entry>::ENTITY,
    icon: "person",
    name_example: "laptop or phone",
    intro: "A person or machine that reaches services with burrow visit.",
    rename_note: "The visitor.yaml on that machine must use the new name, or the visitor cannot connect.",
};

const SERVICE: Info = Info {
    singular: <burrow_config::ServiceEntry as burrow_config::Entry>::ENTITY,
    icon: "stack",
    name_example: "nas or home-ssh",
    intro: "A port on this server, a website, or a service for your visitors, reached through one of your clients.",
    rename_note: "Visitors that forward this service by name must use the new name.",
};

const LOGIN: Info = Info {
    singular: "login service",
    icon: "stack",
    name_example: "authelia or sso",
    intro: "A login service that checks visitors before a website lets them in.",
    rename_note: "Websites that use it are renamed with it.",
};

pub fn info(e: &Entity) -> &'static Info {
    match e {
        Entity::Client => &CLIENT,
        Entity::Visitor => &VISITOR,
        Entity::Service | Entity::Unknown(_) => &SERVICE,
        Entity::Login => &LOGIN,
    }
}

pub fn of_section(section: &str) -> &'static Info {
    burrow_config::entity_of_section(section).map_or(&SERVICE, |e| info(&e))
}

pub const CLIENT_ICON: &str = CLIENT.icon;

#[cfg(test)]
mod tests {
    use super::*;
    use burrow_config::{ClientEntry, Entry, ServiceEntry, VisitorEntry};

    #[test]
    fn every_entity_kind_has_its_words_and_an_unknown_one_reads_as_a_service() {
        for e in [
            Entity::Client,
            Entity::Visitor,
            Entity::Service,
            Entity::Login,
        ] {
            let i = info(&e);
            assert!(!i.singular.is_empty() && !i.icon.is_empty(), "{e:?}");
            assert!(i.name_example.contains(" or "), "{e:?}");
            assert!(
                i.intro.ends_with('.') && i.rename_note.ends_with('.'),
                "{e:?}"
            );
        }
        assert_eq!(info(&Entity::Client).singular, ClientEntry::ENTITY);
        assert_eq!(info(&Entity::Visitor).singular, VisitorEntry::ENTITY);
        assert_eq!(info(&Entity::Service).singular, ServiceEntry::ENTITY);
        assert_eq!(info(&Entity::Unknown("x".into())), &SERVICE);
        assert_eq!(of_section(ClientEntry::section()), &CLIENT);
        assert_eq!(of_section("elsewhere"), &SERVICE);
    }
}

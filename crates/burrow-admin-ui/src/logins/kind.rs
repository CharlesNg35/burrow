use burrow_admin_proto::Path;
use burrow_admin_proto::schema::Entity;
use burrow_admin_proto::serde_json::Value;
use leptos::prelude::*;

use super::Open;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Forward,
    Oidc,
}

pub(crate) struct Ids {
    pub row: &'static str,
    pub cards: &'static str,
    pub none: &'static str,
    pub used: &'static str,
    pub add: &'static str,
    pub add_label: &'static str,
    pub edit: &'static str,
    pub remove: &'static str,
    pub remove_dialog: &'static str,
    pub remove_warning: &'static str,
    pub remove_confirm: &'static str,
}

const FORWARD: Ids = Ids {
    row: "login-services",
    cards: "login-cards",
    none: "logins-none",
    used: "login-used",
    add: "login-add",
    add_label: "Add login service",
    edit: "login-edit",
    remove: "login-remove",
    remove_dialog: "login-remove-dialog",
    remove_warning: "login-remove-warning",
    remove_confirm: "login-remove-confirm",
};

const OIDC: Ids = Ids {
    row: "oidc-providers",
    cards: "oidc-cards",
    none: "oidc-none",
    used: "oidc-used",
    add: "oidc-add",
    add_label: "Add login provider",
    edit: "oidc-edit",
    remove: "oidc-remove",
    remove_dialog: "oidc-remove-dialog",
    remove_warning: "oidc-remove-warning",
    remove_confirm: "oidc-remove-confirm",
};

impl Kind {
    pub(crate) fn of_path(spath: &[String]) -> Option<Kind> {
        [Kind::Forward, Kind::Oidc]
            .into_iter()
            .find(|k| k.base() == spath)
    }

    pub(crate) fn entity(self) -> Entity {
        match self {
            Kind::Forward => Entity::Login,
            Kind::Oidc => Entity::Oidc,
        }
    }

    pub(crate) fn base(self) -> Path {
        burrow_config::entity_path(&self.entity())
    }

    pub(crate) fn what(self) -> &'static str {
        match self {
            Kind::Forward => "login service",
            Kind::Oidc => "login provider",
        }
    }

    pub(crate) fn ids(self) -> &'static Ids {
        match self {
            Kind::Forward => &FORWARD,
            Kind::Oidc => &OIDC,
        }
    }

    pub(crate) fn names(self, doc: &Value) -> Vec<String> {
        let all = match self {
            Kind::Forward => burrow_admin_logic::logins::logins(doc),
            Kind::Oidc => burrow_admin_logic::oidc::providers(doc),
        };
        all.keys().cloned().collect()
    }

    pub(crate) fn used_by(self, doc: &Value, name: &str) -> Vec<String> {
        match self {
            Kind::Forward => burrow_admin_logic::logins::used_by(doc, name),
            Kind::Oidc => burrow_admin_logic::oidc::used_by(doc, name),
        }
    }

    pub(crate) fn body(self, name: &str) -> AnyView {
        match self {
            Kind::Forward => super::forward_body(name),
            Kind::Oidc => super::oidc::card_body(name),
        }
    }

    pub(crate) fn dialog(self, open: RwSignal<Open>) -> AnyView {
        match self {
            Kind::Forward => super::dialog::login_dialog(open).into_any(),
            Kind::Oidc => super::oidc_dialog::oidc_dialog(open).into_any(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_lives_under_web_and_has_its_own_test_ids() {
        assert_eq!(Kind::of_path(&Kind::Oidc.base()), Some(Kind::Oidc));
        assert_eq!(Kind::of_path(&Kind::Forward.base()), Some(Kind::Forward));
        assert_eq!(Kind::of_path(&["web".to_string()]), None);
        let (f, o) = (Kind::Forward.ids(), Kind::Oidc.ids());
        assert_ne!(f.add, o.add);
        assert_ne!(f.remove_dialog, o.remove_dialog);
    }
}

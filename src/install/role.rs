use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Server,
    Client,
    Visitor,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Server => "server",
            Role::Client => "client",
            Role::Visitor => "visitor",
        }
    }

    pub fn parse(word: &str) -> Option<Role> {
        [Role::Server, Role::Client, Role::Visitor]
            .into_iter()
            .find(|r| r.as_str() == word)
    }

    pub fn command(self) -> &'static str {
        match self {
            Role::Visitor => "visit",
            r => r.as_str(),
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

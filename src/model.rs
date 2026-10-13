// SPDX-License-Identifier: MPL-2.0

#[derive(Clone, Debug)]
pub struct Distribution {
    pub id: String,
    pub name: String,
    pub id_like: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    Dnf5,
    Dnf,
    Yum,
    Apt,
    Pacman,
    Paru,
    Yay,
    Zypper,
    RpmOstree,
    Apk,
    Xbps,
    Eopkg,
}

impl Backend {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Dnf5 => "dnf5",
            Self::Dnf => "dnf",
            Self::Yum => "yum",
            Self::Apt => "apt",
            Self::Pacman => "pacman",
            Self::Paru => "paru",
            Self::Yay => "yay",
            Self::Zypper => "zypper",
            Self::RpmOstree => "rpm-ostree",
            Self::Apk => "apk",
            Self::Xbps => "xbps",
            Self::Eopkg => "eopkg",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Dnf5,
            Self::Dnf,
            Self::Yum,
            Self::Apt,
            Self::Pacman,
            Self::Paru,
            Self::Yay,
            Self::Zypper,
            Self::RpmOstree,
            Self::Apk,
            Self::Xbps,
            Self::Eopkg,
        ]
        .into_iter()
        .find(|backend| backend.id() == value)
    }
}

#[derive(Clone, Debug)]
pub struct System {
    pub distro: Distribution,
    pub backend: Option<Backend>,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageUpdate {
    pub name: String,
    pub installed: Option<String>,
    pub available: String,
    pub source: Option<String>,
}

#[derive(Clone, Debug)]
pub struct UpdateReport {
    pub packages: Vec<PackageUpdate>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCommand {
    pub program: String,
    pub args: Vec<String>,
    pub sudo: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdatePlan {
    pub steps: Vec<UpdateCommand>,
}

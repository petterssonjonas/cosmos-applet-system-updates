// SPDX-License-Identifier: MPL-2.0
//! Original distro-inspired symbolic artwork, not official or approved logos.

use cosmic::widget::icon::{self, Handle};

pub fn for_distro(id: &str, id_like: &[String]) -> Handle {
    icon::from_svg_bytes(selected_bytes(id, id_like)).symbolic(true)
}

fn selected_bytes(id: &str, id_like: &[String]) -> &'static [u8] {
    distro_bytes(id)
        .or_else(|| id_like.iter().find_map(|family| distro_bytes(family)))
        .unwrap_or(include_bytes!("../resources/icon.svg"))
}

fn distro_bytes(id: &str) -> Option<&'static [u8]> {
    Some(match id {
        "fedora" => include_bytes!("../resources/icons/fedora.svg"),
        "ubuntu" => include_bytes!("../resources/icons/ubuntu.svg"),
        "arch" | "archlinux" => include_bytes!("../resources/icons/arch.svg"),
        "debian" => include_bytes!("../resources/icons/debian.svg"),
        "pop" | "pop_os" => include_bytes!("../resources/icons/pop.svg"),
        "linuxmint" | "mint" => include_bytes!("../resources/icons/mint.svg"),
        "opensuse" | "opensuse-leap" | "opensuse-tumbleweed" | "suse" | "sles" | "sled" => {
            include_bytes!("../resources/icons/opensuse.svg")
        }
        "manjaro" => include_bytes!("../resources/icons/manjaro.svg"),
        "endeavouros" => include_bytes!("../resources/icons/endeavour.svg"),
        "garuda" => include_bytes!("../resources/icons/garuda.svg"),
        "cachyos" => include_bytes!("../resources/icons/cachyos.svg"),
        "zorin" => include_bytes!("../resources/icons/zorin.svg"),
        "elementary" => include_bytes!("../resources/icons/elementary.svg"),
        "kali" => include_bytes!("../resources/icons/kali.svg"),
        "rhel" | "redhat" => include_bytes!("../resources/icons/rhel.svg"),
        "rocky" => include_bytes!("../resources/icons/rocky.svg"),
        "almalinux" => include_bytes!("../resources/icons/alma.svg"),
        "centos" | "centos-stream" => include_bytes!("../resources/icons/centos.svg"),
        "alpine" => include_bytes!("../resources/icons/alpine.svg"),
        "void" => include_bytes!("../resources/icons/void.svg"),
        "solus" => include_bytes!("../resources/icons/solus.svg"),
        "nixos" => include_bytes!("../resources/icons/nixos.svg"),
        "gentoo" => include_bytes!("../resources/icons/gentoo.svg"),
        "mageia" => include_bytes!("../resources/icons/mageia.svg"),
        "fedora-atomic" | "fedora_atomic" | "silverblue" | "kinoite" | "sericea" | "onyx"
        | "sway-atomic" | "budgie-atomic" | "cosmic-atomic" => {
            include_bytes!("../resources/icons/atomic.svg")
        }
        "bazzite" => include_bytes!("../resources/icons/bazzite.svg"),
        "bluefin" => include_bytes!("../resources/icons/bluefin.svg"),
        "aurora" => include_bytes!("../resources/icons/aurora.svg"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::selected_bytes;

    #[test]
    fn exact_derivative_takes_precedence_over_family() {
        assert_eq!(
            selected_bytes("linuxmint", &["ubuntu".into(), "debian".into()]),
            include_bytes!("../resources/icons/mint.svg").as_slice(),
        );
    }

    #[test]
    fn first_known_family_wins_in_declared_order() {
        assert_eq!(
            selected_bytes(
                "custom",
                &["unknown".into(), "ubuntu".into(), "debian".into()]
            ),
            include_bytes!("../resources/icons/ubuntu.svg").as_slice(),
        );
    }

    #[test]
    fn completely_unknown_distribution_uses_update_icon() {
        assert_eq!(
            selected_bytes("custom", &["linux".into()]),
            include_bytes!("../resources/icon.svg").as_slice(),
        );
    }
}

// SPDX-License-Identifier: MPL-2.0
//! Embedded distro artwork; the Fedora logo retains its original colors.

use cosmic::widget::icon::{self, Handle};

pub fn for_distro(id: &str, id_like: &[String]) -> Handle {
    let (bytes, symbolic) = selected_icon(id, id_like);
    icon::from_svg_bytes(bytes).symbolic(symbolic)
}

fn selected_icon(id: &str, id_like: &[String]) -> (&'static [u8], bool) {
    distro_icon(id)
        .or_else(|| id_like.iter().find_map(|family| distro_icon(family)))
        .unwrap_or((include_bytes!("../resources/icon.svg"), true))
}

fn distro_icon(id: &str) -> Option<(&'static [u8], bool)> {
    let bytes: &'static [u8] = match id {
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
    };
    // Symbolic recoloring would flatten Fedora's blue badge and white mark.
    Some((bytes, id != "fedora"))
}

#[cfg(test)]
mod tests {
    use super::selected_icon;

    #[test]
    fn exact_derivative_takes_precedence_over_family() {
        assert_eq!(
            selected_icon("linuxmint", &["ubuntu".into(), "debian".into()]).0,
            include_bytes!("../resources/icons/mint.svg").as_slice(),
        );
    }

    #[test]
    fn first_known_family_wins_in_declared_order() {
        assert_eq!(
            selected_icon(
                "custom",
                &["unknown".into(), "ubuntu".into(), "debian".into()]
            )
            .0,
            include_bytes!("../resources/icons/ubuntu.svg").as_slice(),
        );
    }

    #[test]
    fn completely_unknown_distribution_uses_update_icon() {
        assert_eq!(
            selected_icon("custom", &["linux".into()]).0,
            include_bytes!("../resources/icon.svg").as_slice(),
        );
    }
}

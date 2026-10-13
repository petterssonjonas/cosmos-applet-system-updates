// SPDX-License-Identifier: MPL-2.0

use std::time::Duration;

use chrono::{DateTime, Local};
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::{Alignment, Length, Limits, Subscription, window::Id};
use cosmic::prelude::*;
use cosmic::widget;

use crate::{
    backend, icons, launcher,
    model::{System, UpdateReport},
};

const WIDTH: f32 = 520.0;
const HEIGHT: f32 = 640.0;
const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
const PAGE_SIZE: usize = 30;

pub struct AppModel {
    core: cosmic::Core,
    popup: Option<Id>,
    monitor: Option<(f32, f32)>,
    system: Option<System>,
    icon: widget::icon::Handle,
    report: Option<UpdateReport>,
    last_success: Option<DateTime<Local>>,
    error: Option<String>,
    notice: Option<String>,
    loading: bool,
    launching: bool,
    page: usize,
}

#[derive(Clone, Debug)]
pub enum Message {
    TogglePopup,
    PopupClosed(Id),
    MonitorSize((f32, f32)),
    Tick,
    Refresh,
    Detected(Result<System, String>),
    Checked(Result<UpdateReport, String>),
    LaunchUpdate,
    Launched(Result<(), String>),
    Page(usize),
    Surface(cosmic::surface::Action<Message>),
}

impl AppModel {
    fn refresh(&mut self) -> Task<cosmic::Action<Message>> {
        if self.loading || self.launching {
            return Task::none();
        }
        self.loading = true;
        self.notice = None;
        Task::perform(async { backend::detect() }, |result| {
            cosmic::Action::App(Message::Detected(result))
        })
    }

    fn size(&self) -> (f32, f32) {
        self.monitor
            .map(|(width, height)| (WIDTH.min(width), HEIGHT.min(height * 0.75)))
            .unwrap_or((WIDTH, HEIGHT))
    }

    fn package_count(&self) -> Option<usize> {
        self.report.as_ref().map(|report| report.packages.len())
    }

    fn summary(&self) -> String {
        match (self.error.is_some(), self.package_count()) {
            (true, Some(count)) => format!("Last result: {count} updates — check failed"),
            (true, None) => "Could not check for updates".into(),
            (false, Some(0)) => "System packages are up to date".into(),
            (false, Some(1)) => "1 update available".into(),
            (false, Some(count)) => format!("{count} updates available"),
            (false, None) => "Checking system updates".into(),
        }
    }

    fn body(&self) -> Element<'_, Message> {
        let mut body = widget::column([]).spacing(12).width(Length::Fill);
        if let Some(system) = &self.system
            && let Some(detail) = &system.detail
        {
            body = body.push(widget::text(detail).size(12));
        }
        if let Some(error) = &self.error {
            body = body.push(
                widget::text(error)
                    .size(12)
                    .class(cosmic::theme::Text::Color(
                        cosmic::theme::active().cosmic().destructive_color().into(),
                    )),
            );
            if self.report.is_some() {
                body = body.push(
                    widget::text(
                        "The list below is from the last successful check; it may be out of date.",
                    )
                    .size(12),
                );
            }
        }
        if let Some(notice) = &self.notice {
            body = body.push(widget::text(notice).size(12));
        }
        if let Some(report) = &self.report {
            if let Some(note) = &report.note {
                body = body.push(widget::text(note).size(12));
            }
            if report.packages.is_empty() {
                body = body.push(
                    widget::container(
                        widget::column([
                            widget::icon::from_name("emblem-ok-symbolic")
                                .size(40)
                                .icon()
                                .into(),
                            widget::text("No system package updates available.")
                                .size(14)
                                .into(),
                            widget::text("Next automatic check within an hour.")
                                .size(12)
                                .into(),
                        ])
                        .spacing(12)
                        .align_x(Alignment::Center),
                    )
                    .width(Length::Fill)
                    .padding([30, 8]),
                );
            }
            for package in report
                .packages
                .iter()
                .skip(self.page * PAGE_SIZE)
                .take(PAGE_SIZE)
            {
                let versions = match &package.installed {
                    Some(installed) => format!("{installed} → {}", package.available),
                    None => package.available.clone(),
                };
                let mut row = widget::column([])
                    .spacing(3)
                    .push(widget::text::heading(&package.name))
                    .push(widget::text(versions).size(12));
                if let Some(source) = &package.source {
                    row = row.push(widget::text(source).size(11));
                }
                body = body.push(row).push(widget::divider::horizontal::default());
            }
        } else if !self.loading && self.error.is_none() {
            body = body.push(widget::text("Use Check now to search for updates.").size(13));
        }
        body.into()
    }
}

impl cosmic::Application for AppModel {
    type Executor = cosmic::executor::Default;
    type Flags = ();
    type Message = Message;
    const APP_ID: &'static str = "io.github.cosmic-applet-system-updates";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }
    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _: ()) -> (Self, Task<cosmic::Action<Message>>) {
        let mut app = Self {
            core,
            popup: None,
            monitor: None,
            system: None,
            icon: icons::for_distro("", &[]),
            report: None,
            last_success: None,
            error: None,
            notice: None,
            loading: false,
            launching: false,
            page: 0,
        };
        let task = app.refresh();
        (app, task)
    }

    fn view(&self) -> Element<'_, Message> {
        let applet = &self.core.applet;
        let suggested = applet.suggested_size(true);
        let (major, minor) = applet.suggested_padding(true);
        let (horizontal, vertical) = if applet.is_horizontal() {
            (major, minor)
        } else {
            (minor, major)
        };
        let mut content = widget::row([widget::icon(self.icon.clone()).size(suggested.0).into()])
            .spacing(5)
            .align_y(Alignment::Center);
        if applet.is_horizontal() {
            if self.error.is_some() {
                content = content.push(applet.text("!"));
            } else if let Some(count) = self.package_count().filter(|count| *count > 0) {
                content = content.push(applet.text(count.to_string()));
            }
        }
        let button = widget::button::custom(
            widget::layer_container(content)
                .center_y(Length::Fixed(f32::from(suggested.1 + 2 * vertical))),
        )
        .name("Open system updates")
        .on_press_down(Message::TogglePopup)
        .padding([0, horizontal])
        .width(Length::Shrink)
        .class(cosmic::theme::Button::AppletIcon);
        applet
            .autosize_window(applet.applet_tooltip(
                button,
                self.summary(),
                self.popup.is_some(),
                Message::Surface,
                self.core.main_window_id(),
            ))
            .into()
    }

    fn view_window(&self, _: Id) -> Element<'_, Message> {
        let (width, height) = self.size();
        let distro = self
            .system
            .as_ref()
            .map_or("Detecting distribution", |system| {
                system.distro.name.as_str()
            });
        let backend = self
            .system
            .as_ref()
            .and_then(|system| system.backend)
            .map_or("No supported backend detected", |backend| backend.id());
        let header = widget::row([
            widget::icon(self.icon.clone()).size(36).into(),
            widget::column([
                widget::text::title3("System Updates").into(),
                widget::text(distro).size(13).into(),
                widget::text(backend).size(11).into(),
            ])
            .spacing(3)
            .width(Length::Fill)
            .into(),
        ])
        .spacing(12)
        .align_y(Alignment::Center);
        let time: Element<'_, Message> = if self.loading {
            widget::row([
                widget::progress_bar::indeterminate_circular()
                    .size(16.0)
                    .into(),
                widget::text("Checking for updates…").size(12).into(),
            ])
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            widget::text(self.last_success.map_or_else(
                || "No successful check yet · checks hourly".into(),
                |time| format!("Last checked {} · checks hourly", time.format("%a %H:%M")),
            ))
            .size(12)
            .into()
        };
        let can_launch = self
            .system
            .as_ref()
            .and_then(|system| system.backend)
            .is_some()
            && !self.loading
            && !self.launching;
        let mut footer = widget::column([]).spacing(10);
        if let Some(count) = self.package_count().filter(|count| *count > PAGE_SIZE) {
            footer = footer.push(
                widget::row([
                    widget::button::standard("Previous")
                        .on_press_maybe((self.page > 0).then(|| Message::Page(self.page - 1)))
                        .into(),
                    widget::text(format!(
                        "{}–{} of {count}",
                        self.page * PAGE_SIZE + 1,
                        ((self.page + 1) * PAGE_SIZE).min(count)
                    ))
                    .size(12)
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .into(),
                    widget::button::standard("Next")
                        .on_press_maybe(
                            ((self.page + 1) * PAGE_SIZE < count)
                                .then_some(Message::Page(self.page + 1)),
                        )
                        .into(),
                ])
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }
        footer = footer.push(
            widget::row([
                widget::button::standard("Check now")
                    .on_press_maybe((!self.loading && !self.launching).then_some(Message::Refresh))
                    .into(),
                widget::Space::new().width(Length::Fill).into(),
                widget::button::suggested(if self.launching {
                    "Opening terminal…"
                } else {
                    "Update in terminal"
                })
                .on_press_maybe(can_launch.then_some(Message::LaunchUpdate))
                .into(),
            ])
            .spacing(8),
        );
        footer = footer
            .push(widget::text("Updates require your confirmation in the terminal.").size(11));
        let content = widget::column([
            header.into(),
            widget::text::heading(self.summary()).into(),
            time,
            widget::divider::horizontal::default().into(),
            widget::scrollable(widget::container(self.body()).padding([0, 8]))
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            widget::divider::horizontal::default().into(),
            footer.into(),
        ])
        .spacing(12)
        .width(Length::Fill);
        self.core
            .applet
            .popup_container(
                widget::container(content)
                    .padding(16)
                    .width(Length::Fill)
                    .height(Length::Fixed(height))
                    .style(|theme| {
                        let background = theme.cosmic().background(false);
                        let mut color: cosmic::iced::Color = background.base.into();
                        color.a = 1.0;
                        widget::container::Style {
                            background: Some(color.into()),
                            text_color: Some(background.on.into()),
                            icon_color: Some(background.on.into()),
                            border: cosmic::iced::Border {
                                radius: theme.cosmic().corner_radii.radius_m.into(),
                                width: 1.0,
                                color: background.divider.into(),
                            },
                            ..Default::default()
                        }
                    }),
            )
            .limits(
                Limits::NONE
                    .min_width(width)
                    .max_width(width)
                    .max_height(height),
            )
            .into()
    }

    fn on_close_requested(&self, id: Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }
    fn on_escape(&mut self) -> Task<cosmic::Action<Message>> {
        if self.popup.is_some() {
            self.update(Message::TogglePopup)
        } else {
            Task::none()
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        use cosmic::iced::core::event::wayland::{Event as WaylandEvent, OutputEvent};
        use cosmic::iced::{Event, event};
        let monitor = event::listen_with(|event, _, _| match event {
            Event::PlatformSpecific(event::PlatformSpecific::Wayland(WaylandEvent::Output(
                OutputEvent::Created(Some(info)) | OutputEvent::InfoUpdate(info),
                _,
            ))) => Some((info.name, info.logical_size)),
            _ => None,
        })
        .with(self.core.applet.output_name.clone())
        .filter_map(|(expected, (name, size))| {
            let (width, height) = size?;
            (name.as_deref() == Some(expected.as_str())
                && !expected.is_empty()
                && width > 0
                && height > 0)
                .then_some(Message::MonitorSize((width as f32, height as f32)))
        });
        Subscription::batch([
            cosmic::iced::time::every(CHECK_INTERVAL).map(|_| Message::Tick),
            monitor,
        ])
    }

    fn update(&mut self, message: Message) -> Task<cosmic::Action<Message>> {
        match message {
            Message::Surface(action) => return cosmic::surface::surface_task(action),
            Message::TogglePopup => {
                if let Some(id) = self.popup.take() {
                    return destroy_popup(id);
                }
                if let Some(parent) = self.core.main_window_id() {
                    let id = Id::unique();
                    self.popup = Some(id);
                    let (width, height) = self.size();
                    let mut settings = self
                        .core
                        .applet
                        .get_popup_settings(parent, id, None, None, None);
                    settings.positioner.size_limits = Limits::NONE
                        .min_width(width)
                        .max_width(width)
                        .max_height(height);
                    return get_popup(settings);
                }
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
            Message::MonitorSize(size) => self.monitor = Some(size),
            Message::Refresh | Message::Tick => return self.refresh(),
            Message::Detected(Ok(system)) => {
                if self.system.as_ref().and_then(|system| system.backend) != system.backend {
                    self.report = None;
                    self.last_success = None;
                }
                self.icon = icons::for_distro(&system.distro.id, &system.distro.id_like);
                self.system = Some(system.clone());
                return Task::perform(async move { backend::check(&system).await }, |result| {
                    cosmic::Action::App(Message::Checked(result))
                });
            }
            Message::Detected(Err(error)) | Message::Checked(Err(error)) => {
                self.loading = false;
                self.error = Some(error);
            }
            Message::Checked(Ok(report)) => {
                self.loading = false;
                self.error = None;
                self.last_success = Some(Local::now());
                self.page = self
                    .page
                    .min(report.packages.len().saturating_sub(1) / PAGE_SIZE);
                self.report = Some(report);
            }
            Message::Page(page) => {
                if page <= self.package_count().unwrap_or(0).saturating_sub(1) / PAGE_SIZE {
                    self.page = page;
                }
            }
            Message::LaunchUpdate if !self.loading && !self.launching => {
                if let Some(backend) = self.system.as_ref().and_then(|system| system.backend) {
                    self.launching = true;
                    self.notice = None;
                    return Task::perform(launcher::launch(backend), |result| {
                        cosmic::Action::App(Message::Launched(result))
                    });
                }
            }
            Message::Launched(result) => {
                self.launching = false;
                self.notice = Some(match result {
                    Ok(()) => {
                        "Updater opened in a terminal. Check again after the update finishes."
                            .into()
                    }
                    Err(error) => format!("Could not open updater: {error}"),
                });
            }
            Message::LaunchUpdate => {}
        }
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Backend, Distribution, PackageUpdate};
    use cosmic::Application;

    fn report(count: usize) -> UpdateReport {
        UpdateReport {
            packages: (0..count)
                .map(|index| PackageUpdate {
                    name: format!("package-{index}"),
                    installed: Some("1.0".into()),
                    available: "2.0".into(),
                    source: None,
                })
                .collect(),
            note: None,
        }
    }

    #[test]
    fn failed_check_preserves_last_result_until_successful_recovery() {
        let (mut app, _) = AppModel::init(cosmic::Core::default(), ());
        let _ = app.update(Message::Checked(Ok(report(2))));
        let checked = app.last_success;
        let _ = app.update(Message::Checked(Err("Repository unavailable".into())));
        assert_eq!(app.package_count(), Some(2));
        assert_eq!(app.last_success, checked);
        assert!(app.error.is_some());
        let _ = app.update(Message::Checked(Ok(report(0))));
        assert_eq!(app.package_count(), Some(0));
        assert!(app.error.is_none());
    }

    #[test]
    fn shrinking_result_keeps_pagination_within_available_packages() {
        let (mut app, _) = AppModel::init(cosmic::Core::default(), ());
        let _ = app.update(Message::Checked(Ok(report(PAGE_SIZE + 1))));
        let _ = app.update(Message::Page(1));
        assert_eq!(app.page, 1);
        let _ = app.update(Message::Checked(Ok(report(2))));
        assert_eq!(app.page, 0);
        let _ = app.update(Message::Page(1));
        assert_eq!(app.page, 0);
    }

    #[test]
    fn backend_change_does_not_relabel_the_previous_backends_result() {
        let (mut app, _) = AppModel::init(cosmic::Core::default(), ());
        let _ = app.update(Message::Checked(Ok(report(2))));
        let _ = app.update(Message::Detected(Ok(System {
            distro: Distribution {
                id: "arch".into(),
                name: "Arch Linux".into(),
                id_like: Vec::new(),
            },
            backend: Some(Backend::Paru),
            detail: None,
        })));
        assert!(app.report.is_none());
        assert!(app.last_success.is_none());
    }
}

use std::{os::fd::AsFd, path::PathBuf};

use adw::prelude::*;
use fraction::Ratio;
use gettextrs::gettext;
use glib::clone;
use gtk::{gdk, gio, glib, subclass::prelude::*};
use itertools::Itertools;

use crate::{
    info::{Dimensions, Framerate},
    profiles::{AudioEncoding, ContainerFormat, OutputFormat, Quality, VideoEncoding},
    runtime, spawn, Listable,
};

mod imp {

    use std::{
        cell::{Cell, RefCell},
        sync::{atomic::AtomicBool, Arc},
    };

    use crate::{
        config::{APP_ID, PKGDATADIR},
        widgets::{preview::VideoPreview, timeline::Timeline},
    };

    use super::*;

    use adw::subclass::prelude::AdwApplicationWindowImpl;
    use derivative::Derivative;
    use gtk::CompositeTemplate;

    #[derive(CompositeTemplate, Derivative)]
    #[derivative(Default)]
    #[template(resource = "/io/gitlab/adhami3310/Footage/blueprints/window.ui")]
    pub struct AppWindow {
        #[template_child]
        pub video_preview: TemplateChild<VideoPreview>,
        #[template_child]
        pub rotate_left_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub rotate_right_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub horizontal_flip_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub vertical_flip_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub audio_button: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub save_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub spinner: TemplateChild<adw::Spinner>,
        #[template_child]
        pub progress_bar: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub try_again_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub done_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub open_result: TemplateChild<gtk::Button>,
        #[template_child]
        pub container_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub video_encoding: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub audio_encoding: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub framerate_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub quality_row: TemplateChild<adw::ComboRow>,
        // #[template_child]
        // pub link_axis: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub resize_type: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub resize_scale_width_value: TemplateChild<gtk::Entry>,
        #[template_child]
        pub resize_scale_height_value: TemplateChild<gtk::Entry>,
        #[template_child]
        pub resize_width_value: TemplateChild<gtk::Entry>,
        #[template_child]
        pub resize_height_value: TemplateChild<gtk::Entry>,
        #[template_child]
        pub cancel_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub back_edit: TemplateChild<gtk::Button>,
        #[template_child]
        pub success_status: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub timeline: TemplateChild<Timeline>,
        #[template_child]
        pub play_pause: TemplateChild<gtk::Button>,
        #[template_child]
        pub open_video_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub editor_split_view: TemplateChild<gtk::Box>,
        #[template_child]
        pub toggle_sidebar_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub settings_sidebar_revealer: TemplateChild<gtk::Revealer>,

        pub running_flag: Arc<AtomicBool>,
        pub video_dimensions: Cell<Option<Dimensions<u32>>>,
        pub selected_video_dimensions: Cell<Option<Dimensions<u32>>>,
        pub selected_video_path: RefCell<Option<PathBuf>>,
        pub result_video_path: RefCell<Option<PathBuf>>,
        pub provider: gtk::CssProvider,
        #[derivative(Default(value = "gio::Settings::new(APP_ID)"))]
        pub settings: gio::Settings,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AppWindow {
        const NAME: &'static str = "AppWindow";
        type Type = super::AppWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }

        fn new() -> Self {
            Self::default()
        }
    }

    impl ObjectImpl for AppWindow {
        fn constructed(&self) {
            self.parent_constructed();

            let theme = gtk::IconTheme::for_display(
                &gtk::gdk::Display::default().expect("cannot find display"),
            );
            theme.add_search_path(PKGDATADIR.to_owned() + "/icons");

            let obj = self.obj();
            obj.load_window_size();
            obj.setup_gactions();
        }
    }

    impl WidgetImpl for AppWindow {}
    impl WindowImpl for AppWindow {
        fn close_request(&self) -> glib::Propagation {
            let obj = self.obj();

            if let Err(err) = obj.save_window_size() {
                dbg!("Failed to save window state, {}", &err);
            }

            if self.running_flag.load(std::sync::atomic::Ordering::SeqCst) {
                self.obj().convert_cancel(true);
                glib::Propagation::Stop
            } else {
                // Pass close request on to the parent
                self.parent_close_request()
            }
        }
    }

    impl ApplicationWindowImpl for AppWindow {}
    impl AdwApplicationWindowImpl for AppWindow {}
}

glib::wrapper! {
    pub struct AppWindow(ObjectSubclass<imp::AppWindow>)
        @extends gtk::Widget, gtk::Window,  gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionMap, gio::ActionGroup,
                    gtk::Root, gtk::Native, gtk::ShortcutManager,
                    gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

#[gtk::template_callbacks]
impl AppWindow {
    fn playback_row(&self) -> gtk::Box {
        self.imp()
            .play_pause
            .parent()
            .and_downcast::<gtk::Box>()
            .expect("play_pause should be inside a Box")
    }

    fn editor_content(&self) -> gtk::Box {
        self.imp()
            .video_preview
            .parent()
            .and_downcast::<gtk::Box>()
            .expect("video_preview should be inside a Box")
    }

    fn path_from_drop_value(value: &glib::Value) -> Option<PathBuf> {
        value
            .get::<gdk::FileList>()
            .ok()
            .and_then(|files| files.files().into_iter().find_map(|file| file.path()))
            .or_else(|| value.get::<gio::File>().ok().and_then(|file| file.path()))
            .or_else(|| {
                value.get::<String>().ok().and_then(|text| {
                    text.lines()
                        .map(str::trim)
                        .find(|line| !line.is_empty() && !line.starts_with('#'))
                        .and_then(|line| url::Url::parse(line).ok())
                        .and_then(|uri| uri.to_file_path().ok())
                })
            })
    }

    pub fn new<P: glib::prelude::IsA<gtk::Application>>(app: &P) -> Self {
        let win = glib::Object::builder::<AppWindow>()
            .property("application", app)
            .build();

        win.setup_callbacks();
        let container_formats = gtk::StringList::new(&[]);

        for cf in ContainerFormat::get_all() {
            container_formats.append(&cf.for_display());
        }

        win.imp().container_row.set_model(Some(
            &ContainerFormat::get_all()
                .into_iter()
                .map(|m| m.for_display())
                .collect_vec()
                .to_list(),
        ));

        win
    }

    fn setup_gactions(&self) {
        self.add_action_entries([
            gio::ActionEntry::builder("close")
                .activate(clone!(
                    #[weak(rename_to=window)]
                    self,
                    move |_, _, _| {
                        window.close();
                    }
                ))
                .build(),
            gio::ActionEntry::builder("about")
                .activate(clone!(
                    #[weak(rename_to=window)]
                    self,
                    move |_, _, _| {
                        window.show_about();
                    }
                ))
                .build(),
            gio::ActionEntry::builder("open")
                .activate(clone!(
                    #[weak(rename_to=window)]
                    self,
                    move |_, _, _| {
                        spawn!(async move {
                            window.open_dialog().await;
                        });
                    }
                ))
                .build(),
        ]);
    }

    fn setup_callbacks(&self) {
        let imp = self.imp();
        let drop_target =
            gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        drop_target.set_types(&[
            gdk::FileList::static_type(),
            gio::File::static_type(),
            String::static_type(),
        ]);
        drop_target.set_preload(true);
        drop_target.connect_enter(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            gdk::DragAction::empty(),
            move |_, _, _| {
                eprintln!("dnd: enter on welcome={:?}", this.imp().stack.visible_child_name());
                if this.imp().stack.visible_child_name().as_deref() == Some("welcome") {
                    this.imp().open_video_button.add_css_class("drop-zone-active");
                    gdk::DragAction::COPY
                } else {
                    gdk::DragAction::empty()
                }
            }
        ));
        drop_target.connect_motion(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            gdk::DragAction::empty(),
            move |_, _, _| {
                eprintln!("dnd: motion on welcome={:?}", this.imp().stack.visible_child_name());
                if this.imp().stack.visible_child_name().as_deref() == Some("welcome") {
                    this.imp().open_video_button.add_css_class("drop-zone-active");
                    gdk::DragAction::COPY
                } else {
                    gdk::DragAction::empty()
                }
            }
        ));
        drop_target.connect_leave(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                eprintln!("dnd: leave");
                this.imp().open_video_button.remove_css_class("drop-zone-active");
            }
        ));
        drop_target.connect_drop(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                this.imp().open_video_button.remove_css_class("drop-zone-active");
                eprintln!("dnd: drop type={}", value.type_().name());

                if this.imp().stack.visible_child_name().as_deref() != Some("welcome") {
                    return false;
                }

                let path = Self::path_from_drop_value(value);
                let Some(path) = path else {
                    eprintln!("dnd: unsupported payload");
                    return false;
                };
                eprintln!("dnd: opening {}", path.display());

                spawn!(async move {
                    this.open_file(path).await;
                });

                true
            }
        ));
        self.add_controller(drop_target);

        imp.rotate_left_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.imp().video_preview.rotate_left();
            }
        ));
        imp.rotate_right_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.imp().video_preview.rotate_right();
            }
        ));
        imp.horizontal_flip_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.imp().video_preview.horizontal_flip();
            }
        ));
        imp.vertical_flip_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.imp().video_preview.vertical_flip();
            }
        ));
        imp.audio_button.connect_toggled(clone!(
            #[weak(rename_to=this)]
            self,
            move |b| {
                if b.is_active() {
                    b.set_icon_name("audio-volume-muted-symbolic");
                    b.set_tooltip_text(Some(&gettext("Enable Audio")));
                } else {
                    b.set_icon_name("audio-volume-high-symbolic");
                    b.set_tooltip_text(Some(&gettext("Disable Audio")));
                }
                // don't think about it
                if b.is_visible() {
                    if b.is_active() {
                        this.imp().video_preview.mute();
                    } else {
                        this.imp().video_preview.unmute();
                    }
                }
            }
        ));
        imp.toggle_sidebar_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                let revealer = &this.imp().settings_sidebar_revealer;
                let new_state = !revealer.reveals_child();
                revealer.set_reveal_child(new_state);
                let button = &this.imp().toggle_sidebar_button;
                if new_state {
                    button.set_icon_name("sidebar-hide-symbolic");
                    button.set_tooltip_text(Some(&gettext("Hide Sidebar")));
                } else {
                    button.set_icon_name("sidebar-show-symbolic");
                    button.set_tooltip_text(Some(&gettext("Show Sidebar")));
                }
            }
        ));
        imp.save_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                spawn!(async move {
                    this.save_dialog().await;
                });
            }
        ));
        imp.try_again_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.imp().video_preview.refresh_ui();
            }
        ));
        imp.done_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.imp().stack.set_visible_child_name("welcome");
                this.imp().back_edit.set_visible(false);
                this.imp().toggle_sidebar_button.set_visible(false);
            }
        ));
        imp.cancel_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.convert_cancel(false);
            }
        ));
        imp.back_edit.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |g| {
                this.imp().video_preview.refresh_ui();
                g.set_visible(false);
            }
        ));
        imp.open_result.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                let file =
                    std::fs::File::open(this.imp().result_video_path.borrow().as_ref().unwrap())
                        .unwrap();
                runtime().spawn(async move {
                    ashpd::desktop::open_uri::OpenFileRequest::default()
                        .ask(true)
                        .send_file(&file.as_fd())
                        .await
                        .ok();
                });
            }
        ));
        imp.container_row.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_options();
            }
        ));
        imp.resize_type.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |rt| {
                match rt.selected() {
                    0 => {
                        this.imp().resize_width_value.set_visible(false);
                        this.imp().resize_height_value.set_visible(false);
                        this.imp().resize_scale_width_value.set_visible(true);
                        this.imp().resize_scale_height_value.set_visible(true);
                    }
                    1 => {
                        this.imp().resize_width_value.set_visible(true);
                        this.imp().resize_height_value.set_visible(true);
                        this.imp().resize_scale_width_value.set_visible(false);
                        this.imp().resize_scale_height_value.set_visible(false);
                    }
                    _ => unreachable!(),
                }
            }
        ));
        imp.resize_width_value.connect_changed(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_height_from_width();
            }
        ));
        imp.resize_height_value.connect_changed(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_width_from_height();
            }
        ));

        imp.resize_scale_height_value.connect_changed(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                // if this.imp().link_axis.is_active() && this.imp().link_axis.is_visible() {
                let old_value = this
                    .imp()
                    .resize_scale_width_value
                    .text()
                    .as_str()
                    .to_owned();
                let new_value = this
                    .imp()
                    .resize_scale_height_value
                    .text()
                    .as_str()
                    .to_owned();
                if old_value != new_value && !new_value.is_empty() {
                    this.imp().resize_scale_width_value.set_text(&new_value);
                }
                // }
            }
        ));

        imp.resize_scale_width_value.connect_changed(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                // if this.imp().link_axis.is_active() && this.imp().link_axis.is_visible() {
                let old_value = this
                    .imp()
                    .resize_scale_height_value
                    .text()
                    .as_str()
                    .to_owned();
                let new_value = this
                    .imp()
                    .resize_scale_width_value
                    .text()
                    .as_str()
                    .to_owned();
                if old_value != new_value && !new_value.is_empty() {
                    this.imp().resize_scale_height_value.set_text(&new_value);
                }
                // }
            }
        ));

        imp.video_preview.imp().crop_box.connect_local(
            "crop-box-changed",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |v| {
                    let (t, r, b, l): (f64, f64, f64, f64) = (
                        v.get(1)?.get().ok()?,
                        v.get(2)?.get().ok()?,
                        v.get(3)?.get().ok()?,
                        v.get(4)?.get().ok()?,
                    );

                    let video_dimensions = this.imp().video_dimensions.get()?;

                    let selected_height =
                        (video_dimensions.height_f64() * (1. - t - b)) as u32 / 2 * 2;
                    let selected_width =
                        (video_dimensions.width_f64() * (1. - l - r)) as u32 / 2 * 2;

                    this.imp().selected_video_dimensions.set(Some(Dimensions {
                        width: selected_width,
                        height: selected_height,
                    }));

                    this.imp()
                        .resize_height_value
                        .set_text(&selected_height.to_string());
                    this.imp()
                        .resize_width_value
                        .set_text(&selected_width.to_string());

                    None
                }
            ),
        );

        imp.video_preview.connect_local(
            "preview-ready",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    this.mark_ui_as_ready();

                    None
                }
            ),
        );

        imp.video_preview.connect_local(
            "orientation-flipped",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    if let Some(video_dimensions) = this.imp().video_dimensions.get() {
                        this.imp()
                            .video_dimensions
                            .set(Some(video_dimensions.swap()));
                    }
                    None
                }
            ),
        );

        imp.timeline.connect_local(
            "set-range",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let values = values.to_vec();
                    let start: u64 = values.get(1).unwrap().get().expect("Expected a U64");
                    let end: u64 = values.get(2).unwrap().get().expect("Expected a U64");
                    if this.imp().video_preview.imp().inpoint.get() != start
                        || this.imp().video_preview.imp().outpoint.get() != end
                    {
                        this.imp().video_preview.set_range(start, end);
                    }
                    None
                }
            ),
        );

        imp.timeline.connect_local(
            "moving",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |_| {
                    this.imp().video_preview.pause();
                    None
                }
            ),
        );

        imp.timeline.connect_local(
            "set-position",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let position: u64 = values[1].get().expect("Expected a U64");

                    this.imp().video_preview.seek(position);

                    None
                }
            ),
        );

        imp.video_preview.connect_local(
            "mode-changed",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let playing: bool = values[1].get().expect("Expected a U64");

                    if playing {
                        this.imp().play_pause.set_icon_name("pause-symbolic");
                        this.imp()
                            .play_pause
                            .set_tooltip_text(Some(&gettext("Pause")));
                    } else {
                        this.imp().play_pause.set_icon_name("play-symbolic");
                        this.imp()
                            .play_pause
                            .set_tooltip_text(Some(&gettext("Play")));
                    }

                    None
                }
            ),
        );

        imp.video_preview.connect_local(
            "set-position",
            true,
            clone!(
                #[weak(rename_to=this)]
                self,
                #[upgrade_or]
                None,
                move |values| {
                    let position: u64 = values[1].get().expect("Expected a U64");

                    this.imp().timeline.set_position(position);

                    None
                }
            ),
        );

        imp.play_pause.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |b| {
                if b.icon_name().unwrap() == "play-symbolic" {
                    this.imp().video_preview.play();
                } else {
                    this.imp().video_preview.pause();
                }
            }
        ));
    }

    fn update_width_from_height(&self) {
        // if self.imp().link_axis.is_active() && self.imp().link_axis.is_visible() {
        if let Some(video_dimensions) = self.imp().selected_video_dimensions.get() {
            let old_value = self.imp().resize_width_value.text().as_str().to_owned();
            let other_text = self.imp().resize_height_value.text().as_str().to_owned();
            if other_text.is_empty() {
                return;
            }

            let other_way =
                generate_height_from_width(old_value.parse().unwrap_or(0), video_dimensions)
                    .to_string();

            if other_way == other_text {
                return;
            }

            let new_value =
                generate_width_from_height(other_text.parse().unwrap_or(0), video_dimensions)
                    .to_string();

            if old_value != new_value && new_value != "0" {
                self.imp().resize_width_value.set_text(&new_value);
            }
        }
        // }
    }

    fn update_height_from_width(&self) {
        // if self.imp().link_axis.is_active() && self.imp().link_axis.is_visible() {
        if let Some(dimensions) = self.imp().selected_video_dimensions.get() {
            let old_value = self.imp().resize_height_value.text().as_str().to_owned();
            let other_text = self.imp().resize_width_value.text().as_str().to_owned();
            if other_text.is_empty() {
                return;
            }

            let other_way =
                generate_width_from_height(old_value.parse().unwrap_or(0), dimensions).to_string();

            if other_way == other_text {
                return;
            }

            let new_value =
                generate_height_from_width(other_text.parse().unwrap_or(0), dimensions).to_string();

            if old_value != new_value && new_value != "0" {
                self.imp().resize_height_value.set_text(&new_value);
            }
        }
        // }
    }

    fn convert_cancel(&self, closing: bool) {
        let stop_converting_dialog = adw::AlertDialog::new(
            Some(&gettext("Stop rendering?")),
            Some(&gettext("You will lose all progress.")),
        );

        stop_converting_dialog
            .add_responses(&[("cancel", &gettext("_Cancel")), ("stop", &gettext("_Stop"))]);
        stop_converting_dialog
            .set_response_appearance("stop", adw::ResponseAppearance::Destructive);

        stop_converting_dialog.connect_response(
            None,
            clone!(
                #[weak(rename_to=this)]
                self,
                move |_, response_id| {
                    if response_id == "stop" {
                        this.imp()
                            .running_flag
                            .store(false, std::sync::atomic::Ordering::SeqCst);

                        if closing {
                            this.close();
                        } else {
                            this.imp().stack.set_visible_child_name("failure");
                        }
                    }
                }
            ),
        );

        stop_converting_dialog.present(Some(self));
    }

    async fn open_dialog(&self) {
        let filter = gtk::FileFilter::new();
        filter.add_mime_type("video/*");
        filter.set_name(Some(&gettext("Video Files")));

        let model = gio::ListStore::new::<gtk::FileFilter>();
        model.append(&filter);

        if let Ok(file) = gtk::FileDialog::builder()
            .modal(true)
            .filters(&model)
            .build()
            .open_future(Some(self))
            .await
        {
            let path = file.path().unwrap();

            self.open_file(path).await;
        }
    }

    async fn save_dialog(&self) {
        let input_path = self.imp().selected_video_path.borrow().to_owned().unwrap();

        let input_path_stem = input_path.file_stem().unwrap().to_str().unwrap().to_owned();

        let extension = match self.selected_container() {
            ContainerFormat::Same => input_path.extension().unwrap().to_str().unwrap().to_owned(),
            x => x.extension().to_owned(),
        };

        if let Ok(file) = gtk::FileDialog::builder()
            .modal(true)
            .initial_name(format!("{}.{}", input_path_stem, extension))
            .build()
            .save_future(Some(self))
            .await
        {
            self.save_file(file.path().unwrap());
        }
    }

    fn selected_container(&self) -> ContainerFormat {
        ContainerFormat::get_all()[self.imp().container_row.selected() as usize]
    }

    fn selected_video_encoding(&self) -> Option<VideoEncoding> {
        let list = self.selected_container().viable_matchings().0;
        if list.is_empty() {
            None
        } else {
            Some(list[self.imp().video_encoding.selected() as usize])
        }
    }

    fn selected_audio_encoding(&self) -> Option<AudioEncoding> {
        let list = self.selected_container().viable_matchings().1;
        if list.is_empty() {
            None
        } else {
            Some(list[self.imp().audio_encoding.selected() as usize])
        }
    }

    fn update_options(&self) {
        let imp = self.imp();

        let selected_container = self.selected_container();

        let (available_video, available_audio) = selected_container.viable_matchings();

        imp.audio_encoding.set_visible(available_audio.len() > 1);
        imp.audio_encoding.set_model(Some(
            &available_audio
                .into_iter()
                .map(|e| e.for_display().to_owned())
                .collect_vec()
                .to_list(),
        ));

        imp.video_encoding.set_visible(available_video.len() > 1);
        imp.video_encoding.set_model(Some(
            &available_video
                .into_iter()
                .map(|e| e.for_display().to_owned())
                .collect_vec()
                .to_list(),
        ));
    }

    fn save_file(&self, path: PathBuf) {
        self.imp().result_video_path.replace(Some(path.clone()));

        let file_name = path.file_name().unwrap().to_str().unwrap().to_owned();

        self.imp()
            .success_status
            .set_description(Some(&gettext("Saved as {}").replace("{}", &file_name)));

        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::None);
        self.imp().stack.set_visible_child_name("exporting");
        glib::MainContext::default().iteration(true);
        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::Crossfade);

        let (scaled_width, scaled_height) = match self.imp().resize_type.selected() {
            0 => {
                let (sw, sh): (u32, u32) = (
                    self.imp().resize_scale_width_value.text().parse().unwrap(),
                    self.imp().resize_scale_height_value.text().parse().unwrap(),
                );

                let selected_video_dimensions = self.imp().selected_video_dimensions.get().unwrap();

                (
                    selected_video_dimensions.width * sw / 100 / 2 * 2,
                    selected_video_dimensions.height * sh / 100 / 2 * 2,
                )
            }
            1 => (
                self.imp().resize_width_value.text().parse::<u32>().unwrap() / 2 * 2,
                self.imp()
                    .resize_height_value
                    .text()
                    .parse::<u32>()
                    .unwrap()
                    / 2
                    * 2,
            ),
            _ => unreachable!(),
        };

        let running_flag = self.imp().running_flag.clone();
        let receiver_running_flag = running_flag.clone();
        running_flag.store(true, std::sync::atomic::Ordering::SeqCst);

        self.imp().progress_bar.set_fraction(0.);

        let (sender, receiver) = async_channel::unbounded();
        self.imp().video_preview.save(
            path,
            sender,
            OutputFormat {
                container_format: self.selected_container(),
                video_encoding: self.selected_video_encoding(),
                audio_encoding: self.selected_audio_encoding(),
                quality: Quality::from_index(self.imp().quality_row.selected()),
            },
            {
                let f = Ratio::<i32>::approximate_float(self.imp().framerate_row.value());

                match f {
                    Some(r) => Framerate {
                        nominator: *r.numer() as u32,
                        denominator: *r.denom() as u32,
                    },
                    _ => Framerate {
                        nominator: 30,
                        denominator: 1,
                    },
                }
            },
            Dimensions {
                width: scaled_width,
                height: scaled_height,
            },
            running_flag,
        );

        glib::spawn_future_local(clone!(
            #[weak(rename_to=this)]
            self,
            async move {
                let mut most_done = 0;
                while let Ok(p) = receiver.recv().await {
                    if !receiver_running_flag.load(std::sync::atomic::Ordering::SeqCst) {
                        this.imp().stack.set_visible_child_name("failure");
                        break;
                    }
                    match p {
                        Ok((done, total)) if done == total => {
                            this.imp().stack.set_visible_child_name("success");
                            this.imp().back_edit.set_visible(true);
                            this.imp()
                                .running_flag
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            break;
                        }
                        Ok((done, total)) => {
                            most_done = std::cmp::max(done, most_done);
                            this.imp()
                                .progress_bar
                                .set_fraction(most_done as f64 / total as f64);
                        }
                        Err(_) => {
                            this.imp().stack.set_visible_child_name("failure");
                            this.imp()
                                .running_flag
                                .store(false, std::sync::atomic::Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
        ));
    }

    async fn create_ui(&self, path: PathBuf) {
        self.imp().video_preview.reset();
        let Ok((dimensions, duration, framerate, has_audio)) =
            self.imp().video_preview.load_path(path).await
        else {
            self.imp().stack.set_visible_child_name("invalid");
            return;
        };
        if has_audio {
            if self.imp().audio_button.is_active() {
                // don't think about it
                self.imp().audio_button.set_visible(false);
                self.imp().audio_button.set_active(false);
            }
            self.imp().audio_button.set_visible(true);
        } else {
            self.imp().audio_button.set_visible(false);
        }
        self.imp().timeline.set_position(0);
        self.imp().timeline.set_duration(duration);
        self.imp().timeline.set_range(Some((0, duration)));
        self.imp().video_dimensions.set(Some(dimensions));
        self.imp().selected_video_dimensions.set(Some(dimensions));
        self.imp().resize_scale_height_value.set_text("100");
        self.imp().resize_scale_width_value.set_text("100");
        self.imp()
            .resize_height_value
            .set_text(&dimensions.height.to_string());
        self.imp()
            .resize_width_value
            .set_text(&dimensions.width.to_string());
        self.imp()
            .framerate_row
            .set_value(framerate.map(|x| x.value().min(240.)).unwrap_or(30.));
    }

    pub fn mark_ui_as_ready(&self) {
        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::Crossfade);
        self.imp().stack.set_visible_child_name("editing");
        self.imp().toggle_sidebar_button.set_visible(true);
        self.imp().play_pause.grab_focus();
    }

    pub async fn open_file(&self, path: PathBuf) {
        self.imp().selected_video_path.replace(Some(path.clone()));

        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::None);
        self.imp().stack.set_visible_child_name("loading");

        self.create_ui(path).await;
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::from_appdata(
            "/io/gitlab/adhami3310/Footage/io.gitlab.adhami3310.Footage.metainfo.xml",
            Some("1.3"),
        );

        about.set_developers(&["Khaleel Al-Adhami"]);
        about.set_artists(&["kramo https://kramo.hu"]);

        // Translators: Replace "translator-credits" with your names, one name per line
        about.set_translator_credits(&gettext("translator-credits"));

        about.present(Some(self));
    }
}

trait SettingsStore {
    fn save_window_size(&self) -> Result<(), glib::BoolError>;
    fn load_window_size(&self);
}

impl SettingsStore for AppWindow {
    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let imp = self.imp();

        let (width, height) = self.default_size();

        imp.settings.set_int("window-width", width)?;
        imp.settings.set_int("window-height", height)?;

        imp.settings
            .set_boolean("is-maximized", self.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let imp = self.imp();

        let width = imp.settings.int("window-width");
        let height = imp.settings.int("window-height");
        let is_maximized = imp.settings.boolean("is-maximized");

        self.set_default_size(width, height);

        if is_maximized {
            self.maximize();
        }
    }
}

fn generate_width_from_height(height: u32, image_dim: Dimensions<u32>) -> u32 {
    ((height as f64) * (image_dim.width_f64()) / (image_dim.height_f64())).round() as u32
}

fn generate_height_from_width(width: u32, image_dim: Dimensions<u32>) -> u32 {
    ((width as f64) * (image_dim.height_f64()) / (image_dim.width_f64())).round() as u32
}

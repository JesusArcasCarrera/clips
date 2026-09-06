use std::{os::fd::AsFd, path::PathBuf};

use adw::prelude::*;
use fraction::Ratio;
use gettextrs::gettext;
use glib::clone;
use gtk::{gdk, gio, glib, subclass::prelude::*};
use itertools::Itertools;

use crate::{
    adjustments::{ColorAdjustments, PlaybackSpeed, Repeat, RepeatMode},
    info::{get_duration_ms, get_info, Dimensions, Framerate},
    profiles::{AudioEncoding, ContainerFormat, OutputFormat, Quality, VideoEncoding},
    runtime,
    segments::{ClipRange, ClipSegment, SegmentExportMode},
    spawn, Listable,
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
    #[template(resource = "/io/gitlab/adhami3310/Clips/blueprints/window.ui")]
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
        pub reveal_result: TemplateChild<gtk::Button>,
        #[template_child]
        pub edit_result: TemplateChild<gtk::Button>,
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
        #[template_child]
        pub gpu_row: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub encoder_status_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub encoder_status_icon: TemplateChild<gtk::Image>,
        // #[template_child]
        // pub link_axis: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub resize_type: TemplateChild<gtk::DropDown>,
        #[template_child]
        pub resize_amount_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub enhance_quality_row: TemplateChild<adw::SwitchRow>,
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
        #[template_child]
        pub adjustments_row: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub adjust_brightness: TemplateChild<gtk::Scale>,
        #[template_child]
        pub adjust_contrast: TemplateChild<gtk::Scale>,
        #[template_child]
        pub adjust_saturation: TemplateChild<gtk::Scale>,
        #[template_child]
        pub adjust_hue: TemplateChild<gtk::Scale>,
        #[template_child]
        pub adjust_gamma: TemplateChild<gtk::Scale>,
        #[template_child]
        pub adjust_sharpness: TemplateChild<gtk::Scale>,
        #[template_child]
        pub adjustments_reset: TemplateChild<adw::ButtonRow>,
        #[template_child]
        pub repeat_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub speed_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub repeat_count_row: TemplateChild<adw::SpinRow>,
        #[template_child]
        pub add_segment_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub add_source_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub move_segment_up_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub move_segment_down_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub remove_segment_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub segment_export_mode: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub sections_list: TemplateChild<gtk::ListBox>,

        pub running_flag: Arc<AtomicBool>,
        pub video_dimensions: Cell<Option<Dimensions<u32>>>,
        pub selected_video_dimensions: Cell<Option<Dimensions<u32>>>,
        pub selected_video_path: RefCell<Option<PathBuf>>,
        pub result_video_path: RefCell<Option<PathBuf>>,
        pub segments: RefCell<Vec<ClipSegment>>,
        pub active_segment: Cell<usize>,
        pub updating_segments: Cell<bool>,
        #[derivative(Default(value = "Cell::new(true)"))]
        pub gpu_preference: Cell<bool>,
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

        win.imp().repeat_row.set_model(Some(
            &RepeatMode::get_all()
                .into_iter()
                .map(|m| m.for_display())
                .collect_vec()
                .to_list(),
        ));

        win.imp().speed_row.set_model(Some(
            &PlaybackSpeed::get_all()
                .into_iter()
                .map(|speed| speed.for_display())
                .collect_vec()
                .to_list(),
        ));

        win.imp().segment_export_mode.set_model(Some(
            &SegmentExportMode::get_all()
                .into_iter()
                .map(|mode| mode.for_display())
                .collect_vec()
                .to_list(),
        ));

        win.update_options();

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
        let drop_target = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
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
                eprintln!(
                    "dnd: enter on welcome={:?}",
                    this.imp().stack.visible_child_name()
                );
                if this.imp().stack.visible_child_name().as_deref() == Some("welcome") {
                    this.imp()
                        .open_video_button
                        .add_css_class("drop-zone-active");
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
                eprintln!(
                    "dnd: motion on welcome={:?}",
                    this.imp().stack.visible_child_name()
                );
                if this.imp().stack.visible_child_name().as_deref() == Some("welcome") {
                    this.imp()
                        .open_video_button
                        .add_css_class("drop-zone-active");
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
                this.imp()
                    .open_video_button
                    .remove_css_class("drop-zone-active");
            }
        ));
        drop_target.connect_drop(clone!(
            #[weak(rename_to = this)]
            self,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                this.imp()
                    .open_video_button
                    .remove_css_class("drop-zone-active");
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
        imp.edit_result.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.return_to_editing();
            }
        ));
        imp.open_result.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.open_exported_result();
            }
        ));
        imp.reveal_result.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.show_export_folder();
            }
        ));
        for scale in [
            &imp.adjust_brightness,
            &imp.adjust_contrast,
            &imp.adjust_saturation,
            &imp.adjust_hue,
            &imp.adjust_gamma,
            &imp.adjust_sharpness,
        ] {
            scale.connect_value_changed(clone!(
                #[weak(rename_to=this)]
                self,
                move |_| {
                    this.apply_color_adjustments();
                }
            ));
        }
        imp.adjustments_reset.connect_activated(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.reset_color_adjustments();
            }
        ));
        imp.repeat_row.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |row| {
                // A loop of one cycle is a no-op, while a single boomerang cycle is
                // already forwards+backwards — so they want different defaults.
                let count = match RepeatMode::from_index(row.selected()) {
                    RepeatMode::Boomerang => 1.,
                    _ => 2.,
                };
                this.imp().repeat_count_row.set_value(count);
                this.update_repeat_ui();
                this.update_speed_ui();
            }
        ));
        imp.repeat_count_row.connect_value_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_repeat_ui();
                this.update_speed_ui();
            }
        ));
        imp.speed_row.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_speed_ui();
            }
        ));
        imp.sections_list.connect_row_selected(clone!(
            #[weak(rename_to=this)]
            self,
            move |_, row| {
                if !this.imp().updating_segments.get() {
                    if let Some(row) = row {
                        this.select_segment(row.index() as usize);
                    }
                }
            }
        ));
        imp.add_segment_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.add_segment_at_playhead();
            }
        ));
        imp.add_source_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                spawn!(async move {
                    this.add_source_dialog().await;
                });
            }
        ));
        imp.move_segment_up_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.move_active_segment(-1);
            }
        ));
        imp.move_segment_down_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.move_active_segment(1);
            }
        ));
        imp.remove_segment_button.connect_clicked(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.remove_active_segment();
            }
        ));
        imp.segment_export_mode.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_segments_ui();
            }
        ));
        imp.container_row.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_options();
            }
        ));
        imp.video_encoding.connect_selected_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_encoder_status();
            }
        ));
        imp.gpu_row.connect_active_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |row| {
                if row.is_sensitive() {
                    this.imp().gpu_preference.set(row.is_active());
                }
                this.update_encoder_status();
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
        imp.enhance_quality_row.connect_active_notify(clone!(
            #[weak(rename_to=this)]
            self,
            move |_| {
                this.update_enhance_quality_ui();
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
                    this.update_active_segment(start, end);
                    // The trimmed length drives the loop/boomerang length hint.
                    this.update_repeat_ui();
                    this.update_speed_ui();
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

        if let Ok(files) = gtk::FileDialog::builder()
            .modal(true)
            .filters(&model)
            .build()
            .open_multiple_future(Some(self))
            .await
        {
            let paths = file_paths(&files);
            self.open_files(paths).await;
        }
    }

    async fn add_source_dialog(&self) {
        let filter = gtk::FileFilter::new();
        filter.add_mime_type("video/*");
        filter.set_name(Some(&gettext("Video Files")));
        let model = gio::ListStore::new::<gtk::FileFilter>();
        model.append(&filter);

        if let Ok(files) = gtk::FileDialog::builder()
            .modal(true)
            .title(gettext("Add Video Sources"))
            .filters(&model)
            .build()
            .open_multiple_future(Some(self))
            .await
        {
            for path in file_paths(&files) {
                self.append_source(path).await;
            }
        }
    }

    async fn append_source(&self, path: PathBuf) {
        let Some(duration_ms) = get_duration_ms(&path) else {
            log::warn!(
                "ignoring source without a readable duration: {}",
                path.display()
            );
            return;
        };
        if get_info(path.to_string_lossy().into_owned()).is_none() {
            log::warn!(
                "ignoring source without a readable video stream: {}",
                path.display()
            );
            return;
        }

        self.imp()
            .segments
            .borrow_mut()
            .push(ClipSegment::new(path, 0, duration_ms));
        self.refresh_sequence_preview().await;
        self.update_segments_ui();
    }

    async fn refresh_sequence_preview(&self) {
        let segments = self.imp().segments.borrow().clone();
        if segments.is_empty() {
            return;
        }
        if self
            .imp()
            .video_preview
            .load_sequence(&segments)
            .await
            .is_err()
        {
            log::warn!("could not load the complete clip sequence for preview");
        }
        let active = self
            .imp()
            .active_segment
            .get()
            .min(segments.len().saturating_sub(1));
        self.imp().video_preview.set_active_segment(active);
    }

    async fn save_dialog(&self) {
        let input_path = self.imp().selected_video_path.borrow().to_owned().unwrap();

        if self.selected_segment_export_mode() == SegmentExportMode::Separate {
            if let Ok(folder) = gtk::FileDialog::builder()
                .modal(true)
                .title(gettext("Choose Where to Save the Clips"))
                .build()
                .select_folder_future(Some(self))
                .await
            {
                if let Some(path) = folder.path() {
                    self.save_file(path);
                }
            }
            return;
        }

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

    /// The colour correction currently dialled in on the sliders.
    fn color_adjustments(&self) -> ColorAdjustments {
        let imp = self.imp();

        ColorAdjustments {
            brightness: imp.adjust_brightness.value(),
            contrast: imp.adjust_contrast.value(),
            saturation: imp.adjust_saturation.value(),
            hue: imp.adjust_hue.value(),
            gamma: imp.adjust_gamma.value(),
            sharpness: imp.adjust_sharpness.value(),
        }
    }

    fn apply_color_adjustments(&self) {
        let adjustments = self.color_adjustments();

        self.imp()
            .adjustments_row
            .set_subtitle(&if adjustments.is_neutral() {
                gettext("Brightness, contrast, colour, sharpness")
            } else {
                gettext("Modified")
            });

        self.imp().video_preview.set_color_adjustments(adjustments);
    }

    fn reset_color_adjustments(&self) {
        let imp = self.imp();
        let neutral = ColorAdjustments::NEUTRAL;

        imp.adjust_brightness.set_value(neutral.brightness);
        imp.adjust_contrast.set_value(neutral.contrast);
        imp.adjust_saturation.set_value(neutral.saturation);
        imp.adjust_hue.set_value(neutral.hue);
        imp.adjust_gamma.set_value(neutral.gamma);
        imp.adjust_sharpness.set_value(neutral.sharpness);

        self.apply_color_adjustments();
    }

    fn multi_segments_supported(&self) -> bool {
        !matches!(
            self.selected_container(),
            ContainerFormat::Same | ContainerFormat::GifContainer
        )
    }

    fn update_enhance_quality_ui(&self) {
        let imp = self.imp();
        let supported = self.multi_segments_supported();
        if !supported && imp.enhance_quality_row.is_active() {
            imp.enhance_quality_row.set_active(false);
        }

        imp.enhance_quality_row.set_sensitive(supported);
        imp.enhance_quality_row.set_subtitle(&if supported {
            gettext("Export at 2× with high-quality upscaling")
        } else {
            gettext("Choose MP4, WebM or Matroska in Export")
        });
        imp.resize_amount_row
            .set_sensitive(!imp.enhance_quality_row.is_active());
        imp.resize_amount_row
            .set_subtitle(&if imp.enhance_quality_row.is_active() {
                gettext("Controlled by Improve Quality · 200%")
            } else {
                String::new()
            });
    }

    fn reset_segments(&self, duration_ms: u64) {
        let source = self
            .imp()
            .selected_video_path
            .borrow()
            .clone()
            .unwrap_or_default();
        self.imp()
            .segments
            .replace(vec![ClipSegment::new(source, 0, duration_ms)]);
        self.imp().active_segment.set(0);
        self.update_segments_ui();
    }

    fn update_active_segment(&self, start_ms: u64, end_ms: u64) {
        let imp = self.imp();
        let active = imp.active_segment.get();
        if let Some(segment) = imp.segments.borrow_mut().get_mut(active) {
            segment.range = ClipRange::new(start_ms, end_ms);
        }
        self.update_segments_ui();
    }

    fn add_segment_at_playhead(&self) {
        if !self.multi_segments_supported() {
            return;
        }

        let duration = self.imp().timeline.duration();
        if duration == 0 {
            return;
        }

        const DEFAULT_SECTION_MS: u64 = 5_000;
        let position = self.imp().timeline.position().min(duration);
        let (start, end) = if position + DEFAULT_SECTION_MS <= duration {
            (position, position + DEFAULT_SECTION_MS)
        } else {
            (duration.saturating_sub(DEFAULT_SECTION_MS), duration)
        };

        let index = {
            let mut segments = self.imp().segments.borrow_mut();
            let source = segments
                .first()
                .map(|segment| segment.source.clone())
                .unwrap_or_default();
            segments.push(ClipSegment::new(source, start, end));
            segments.len() - 1
        };
        self.select_segment(index);
        self.update_segments_ui();
        let this = self.clone();
        spawn!(async move {
            this.refresh_sequence_preview().await;
        });
    }

    fn remove_active_segment(&self) {
        let imp = self.imp();
        if imp.segments.borrow().len() <= 1 {
            return;
        }

        let index = imp.active_segment.get();
        imp.segments.borrow_mut().remove(index);
        let next = index.min(imp.segments.borrow().len() - 1);
        self.select_segment(next);
        self.update_segments_ui();
        let this = self.clone();
        spawn!(async move {
            this.refresh_sequence_preview().await;
        });
    }

    fn move_active_segment(&self, direction: isize) {
        let mut segments = self.imp().segments.borrow_mut();
        let index = self.imp().active_segment.get();
        let target = index as isize + direction;
        if target < 0 || target as usize >= segments.len() {
            return;
        }
        segments.swap(index, target as usize);
        self.imp().active_segment.set(target as usize);
        drop(segments);
        self.update_segments_ui();
        let this = self.clone();
        spawn!(async move {
            this.refresh_sequence_preview().await;
        });
    }

    fn select_segment(&self, index: usize) {
        let imp = self.imp();
        let segment = {
            let segments = imp.segments.borrow();
            segments.get(index).cloned()
        };
        let Some(segment) = segment else {
            return;
        };

        imp.active_segment.set(index);
        imp.video_preview.set_active_segment(index);
        imp.video_preview.pause();
        if let Some(duration_ms) = get_duration_ms(segment.source_path()) {
            imp.timeline.set_duration(duration_ms);
        }
        imp.timeline
            .set_range(Some((segment.range.start_ms, segment.range.end_ms)));
        imp.timeline.set_position(segment.range.start_ms);
        imp.video_preview
            .set_range(segment.range.start_ms, segment.range.end_ms);
        imp.video_preview.seek(segment.range.start_ms);

        imp.updating_segments.set(true);
        if let Some(row) = imp.sections_list.row_at_index(index as i32) {
            imp.sections_list.select_row(Some(&row));
        }
        imp.updating_segments.set(false);
        self.update_repeat_ui();
    }

    fn selected_segment_export_mode(&self) -> SegmentExportMode {
        if self.imp().segments.borrow().len() <= 1 {
            SegmentExportMode::Join
        } else {
            SegmentExportMode::from_index(self.imp().segment_export_mode.selected())
        }
    }

    fn update_segments_ui(&self) {
        let imp = self.imp();
        let segments = imp.segments.borrow();

        imp.updating_segments.set(true);
        while let Some(child) = imp.sections_list.first_child() {
            imp.sections_list.remove(&child);
        }
        for (index, segment) in segments.iter().enumerate() {
            let range = segment.range;
            let row = adw::ActionRow::builder()
                .title(
                    gettext("{} · Section {}")
                        .replacen("{}", segment.source_name(), 1)
                        .replacen("{}", &(index + 1).to_string(), 1),
                )
                .subtitle(
                    gettext("{} – {} · {}")
                        .replacen("{}", &format_duration_ms(range.start_ms), 1)
                        .replacen("{}", &format_duration_ms(range.end_ms), 1)
                        .replacen("{}", &format_duration_ms(range.duration_ms()), 1),
                )
                .activatable(true)
                .build();
            imp.sections_list.append(&row);
        }
        if !segments.is_empty() {
            if let Some(row) = imp
                .sections_list
                .row_at_index(imp.active_segment.get().min(segments.len() - 1) as i32)
            {
                imp.sections_list.select_row(Some(&row));
            }
        }
        imp.updating_segments.set(false);

        let multiple = segments.len() > 1;
        let supported = self.multi_segments_supported();
        let valid = segments.iter().all(|segment| segment.duration_ms() > 0);
        imp.remove_segment_button.set_sensitive(multiple);
        imp.move_segment_up_button
            .set_sensitive(multiple && imp.active_segment.get() > 0);
        imp.move_segment_down_button
            .set_sensitive(multiple && imp.active_segment.get().saturating_add(1) < segments.len());
        imp.segment_export_mode.set_visible(multiple);
        imp.add_segment_button.set_sensitive(supported);
        imp.add_segment_button.set_tooltip_text(Some(&if supported {
            gettext("Add Section at Playhead")
        } else {
            gettext("Multiple sections are not available for this container format")
        }));

        imp.save_button
            .set_sensitive(valid && (!multiple || supported));
        let save_tooltip = if valid && (!multiple || supported) {
            None
        } else if !valid {
            Some(gettext("Every section must have a duration"))
        } else {
            Some(gettext(
                "Choose a container format that supports multiple sections",
            ))
        };
        imp.save_button.set_tooltip_text(save_tooltip.as_deref());
        imp.save_button.set_label(&if multiple
            && self.selected_segment_export_mode() == SegmentExportMode::Separate
        {
            gettext("Save Clips")
        } else {
            gettext("Save Video")
        });

        drop(segments);
        self.update_repeat_ui();
    }

    /// Whether the selected container can be exported through the ffmpeg render path,
    /// which is the only one that implements loop and boomerang.
    fn repeat_is_supported(&self) -> bool {
        self.imp().segments.borrow().len() <= 1
            && !matches!(
                self.selected_container(),
                ContainerFormat::Same | ContainerFormat::GifContainer
            )
    }

    fn selected_repeat(&self) -> Repeat {
        if !self.repeat_is_supported() {
            return Repeat::OFF;
        }

        Repeat {
            mode: RepeatMode::from_index(self.imp().repeat_row.selected()),
            count: self.imp().repeat_count_row.value().round().max(1.) as u32,
        }
    }

    fn speed_is_supported(&self) -> bool {
        self.multi_segments_supported()
    }

    fn selected_playback_speed(&self) -> PlaybackSpeed {
        if self.speed_is_supported() {
            PlaybackSpeed::from_index(self.imp().speed_row.selected())
        } else {
            PlaybackSpeed::Normal
        }
    }

    fn update_speed_ui(&self) {
        let imp = self.imp();
        let supported = self.speed_is_supported();
        imp.speed_row.set_sensitive(supported);

        let speed = self.selected_playback_speed();
        imp.video_preview.set_playback_rate(speed.factor());

        let subtitle = if !supported {
            gettext("Not available for this container format")
        } else if speed.is_normal() {
            gettext("Change video and audio speed")
        } else {
            let source_ms = imp
                .segments
                .borrow()
                .iter()
                .map(ClipSegment::duration_ms)
                .sum::<u64>();
            let slowed_ms = speed.output_duration_ms(source_ms);
            let output_ms = self.selected_repeat().output_duration_ms(slowed_ms);
            gettext("Resulting length: {}").replace("{}", &format_duration_ms(output_ms))
        };
        imp.speed_row.set_subtitle(&subtitle);
    }

    /// Keeps the repeat rows in sync: the cycle count only matters when a mode is
    /// picked, and the hint spells out how long the export will end up being.
    fn update_repeat_ui(&self) {
        let imp = self.imp();

        let supported = self.repeat_is_supported();
        imp.repeat_row.set_sensitive(supported);
        imp.repeat_row.set_subtitle(&if supported {
            String::new()
        } else if imp.segments.borrow().len() > 1 {
            gettext("Not available with multiple sections")
        } else {
            gettext("Not available for this container format")
        });

        let repeat = self.selected_repeat();
        let active = supported && repeat.mode != RepeatMode::Off;
        imp.repeat_count_row.set_visible(active);

        if !active {
            return;
        }

        let selection_ms = imp
            .video_preview
            .imp()
            .outpoint
            .get()
            .saturating_sub(imp.video_preview.imp().inpoint.get());
        let selection_ms = self
            .selected_playback_speed()
            .output_duration_ms(selection_ms);

        imp.repeat_count_row
            .set_subtitle(&gettext("Resulting length: {}").replace(
                "{}",
                &format_duration_ms(repeat.output_duration_ms(selection_ms)),
            ));
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

        self.update_encoder_status();
        self.update_segments_ui();
        self.update_enhance_quality_ui();
        self.update_speed_ui();
    }

    fn return_to_editing(&self) {
        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::None);
        self.imp().stack.set_visible_child_name("loading");
        self.imp().video_preview.refresh_ui();
    }

    fn open_exported_result(&self) {
        let Some(path) = self.imp().result_video_path.borrow().clone() else {
            return;
        };

        runtime().spawn(async move {
            let Ok(file) = std::fs::File::open(&path) else {
                log::error!("could not open exported result: {}", path.display());
                return;
            };

            if path.is_dir() {
                if let Err(err) = ashpd::desktop::open_uri::OpenDirectoryRequest::default()
                    .send(&file.as_fd())
                    .await
                {
                    log::error!("could not open export directory: {err}");
                }
            } else if let Err(err) = ashpd::desktop::open_uri::OpenFileRequest::default()
                .ask(true)
                .send_file(&file.as_fd())
                .await
            {
                log::error!("could not open exported file: {err}");
            }
        });
    }

    fn show_export_folder(&self) {
        let Some(result_path) = self.imp().result_video_path.borrow().clone() else {
            return;
        };
        let folder = if result_path.is_dir() {
            result_path
        } else {
            result_path
                .parent()
                .map(PathBuf::from)
                .unwrap_or(result_path)
        };

        runtime().spawn(async move {
            let Ok(directory) = std::fs::File::open(&folder) else {
                log::error!("could not open export folder: {}", folder.display());
                return;
            };
            if let Err(err) = ashpd::desktop::open_uri::OpenDirectoryRequest::default()
                .send(&directory.as_fd())
                .await
            {
                log::error!("could not show export folder: {err}");
            }
        });
    }

    /// Updates the "Encoder" row to clearly show whether the export will run on the
    /// GPU (NVENC) or the CPU (software), based on the selected codec and the
    /// hardware-acceleration switch.
    fn update_encoder_status(&self) {
        let imp = self.imp();

        let Some(video_encoding) = self.selected_video_encoding() else {
            imp.encoder_status_row.set_visible(false);
            imp.gpu_row.set_visible(false);
            return;
        };

        imp.encoder_status_row.set_visible(true);
        imp.gpu_row.set_visible(true);

        let ffmpeg_path = self.multi_segments_supported();
        let (software_name, ffmpeg_hardware) = video_encoding.ffmpeg_encoders();
        let (hardware_name, hardware_available) = if ffmpeg_path {
            (
                ffmpeg_hardware,
                ffmpeg_hardware
                    .map(crate::widgets::preview::ffmpeg_has_encoder)
                    .unwrap_or(false),
            )
        } else {
            let resolved = video_encoding.resolve_encoder(true);
            (
                resolved.hardware.then_some(resolved.element),
                resolved.hardware,
            )
        };

        imp.gpu_row.set_sensitive(hardware_available);
        imp.gpu_row
            .set_active(hardware_available && imp.gpu_preference.get());
        imp.gpu_row
            .set_subtitle(&match (hardware_name, hardware_available) {
                (None, _) => gettext("Not supported by the selected video codec"),
                (Some(_), false) => gettext("No compatible hardware encoder was found"),
                (Some(_), true) => gettext("Use the available hardware encoder"),
            });

        let hardware_active = hardware_available && imp.gpu_row.is_active();
        let encoder_label = if hardware_active {
            format!("GPU · NVENC ({})", hardware_name.unwrap())
        } else if ffmpeg_path {
            format!("CPU · {software_name} (software)")
        } else {
            video_encoding.resolve_encoder(false).label()
        };
        imp.encoder_status_row.set_subtitle(&encoder_label);
        imp.encoder_status_icon
            .set_icon_name(Some(if hardware_active {
                "power-profile-performance-symbolic"
            } else {
                "power-profile-power-saver-symbolic"
            }));
    }

    fn save_file(&self, path: PathBuf) {
        self.imp().result_video_path.replace(Some(path.clone()));

        let segments = self.imp().segments.borrow().clone();
        let segment_export_mode = self.selected_segment_export_mode();
        let target_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();

        self.imp()
            .success_status
            .set_description(Some(
                &if segment_export_mode == SegmentExportMode::Separate {
                    gettext("Saved {} clips in {}")
                        .replacen("{}", &segments.len().to_string(), 1)
                        .replacen("{}", &target_name, 1)
                } else {
                    gettext("Saved as {}").replace("{}", &target_name)
                },
            ));

        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::None);
        self.imp().toggle_sidebar_button.set_visible(false);
        self.imp().stack.set_visible_child_name("exporting");
        glib::MainContext::default().iteration(true);
        self.imp()
            .stack
            .set_transition_type(gtk::StackTransitionType::Crossfade);

        let (scaled_width, scaled_height) = if self.imp().enhance_quality_row.is_active() {
            let dimensions = self.imp().selected_video_dimensions.get().unwrap();
            (
                dimensions.width.saturating_mul(2) / 2 * 2,
                dimensions.height.saturating_mul(2) / 2 * 2,
            )
        } else {
            match self.imp().resize_type.selected() {
                0 => {
                    let (sw, sh): (u32, u32) = (
                        self.imp().resize_scale_width_value.text().parse().unwrap(),
                        self.imp().resize_scale_height_value.text().parse().unwrap(),
                    );

                    let selected_video_dimensions =
                        self.imp().selected_video_dimensions.get().unwrap();

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
            }
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
            self.imp().gpu_row.is_active(),
            self.selected_repeat(),
            self.selected_playback_speed(),
            segments,
            segment_export_mode,
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
        self.reset_color_adjustments();
        self.imp().speed_row.set_selected(0);
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
        self.reset_segments(duration);
        self.refresh_sequence_preview().await;
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
        self.update_repeat_ui();
        self.update_speed_ui();
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

    pub async fn open_files(&self, paths: Vec<PathBuf>) {
        let Some(first) = paths.first().cloned() else {
            return;
        };
        self.open_file(first).await;
        for path in paths.into_iter().skip(1) {
            self.append_source(path).await;
        }
    }

    fn show_about(&self) {
        let about = adw::AboutDialog::from_appdata(
            "/io/gitlab/adhami3310/Clips/io.gitlab.adhami3310.Clips.metainfo.xml",
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

/// `m:ss`, or `h:mm:ss` once it passes an hour.
fn format_duration_ms(ms: u64) -> String {
    let total_seconds = (ms + 999) / 1000;
    let (hours, minutes, seconds) = (
        total_seconds / 3600,
        (total_seconds / 60) % 60,
        total_seconds % 60,
    );

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn generate_width_from_height(height: u32, image_dim: Dimensions<u32>) -> u32 {
    ((height as f64) * (image_dim.width_f64()) / (image_dim.height_f64())).round() as u32
}

fn generate_height_from_width(width: u32, image_dim: Dimensions<u32>) -> u32 {
    ((width as f64) * (image_dim.height_f64()) / (image_dim.width_f64())).round() as u32
}

fn file_paths(files: &gio::ListModel) -> Vec<PathBuf> {
    (0..files.n_items())
        .filter_map(|index| files.item(index))
        .filter_map(|item| item.downcast::<gio::File>().ok())
        .filter_map(|file| file.path())
        .collect()
}

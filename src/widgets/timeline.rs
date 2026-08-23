// https://gitlab.gnome.org/YaLTeR/video-trimmer/-/blob/master/src/timeline.rs

use glib::subclass::prelude::*;
use gtk::glib;

mod imp {
    use super::*;
    use glib::{clone, subclass::Signal};
    use gtk::{
        gdk::{self, Key},
        prelude::*,
        subclass::prelude::*,
        CompositeTemplate,
    };
    use once_cell::unsync::OnceCell;
    use std::cell::Cell;

    const TOLERANCE: f64 = 12.;
    const TIMELINE_KEYBOARD_MOVE: i64 = 250;

    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    enum DragType {
        Playback,
        Start,
        End,
    }

    #[derive(Debug, Clone, Copy, Eq, PartialEq)]
    enum CursorType {
        Normal,
        StartEnd,
    }

    impl CursorType {
        fn gtk_cursor_name(self) -> &'static str {
            match self {
                CursorType::Normal => "default",
                CursorType::StartEnd => "col-resize",
            }
        }
    }

    #[derive(Debug, CompositeTemplate)]
    #[template(resource = "/io/gitlab/adhami3310/Clips/blueprints/timeline.ui")]
    pub struct Timeline {
        #[template_child]
        box_timeline_position: TemplateChild<gtk::Box>,
        #[template_child]
        box_timeline_selection: TemplateChild<gtk::Box>,
        #[template_child]
        box_wrapper: TemplateChild<gtk::Box>,
        #[template_child]
        left_handle: TemplateChild<gtk::Button>,
        #[template_child]
        right_handle: TemplateChild<gtk::Button>,

        position: Cell<u64>,
        duration: Cell<u64>,
        range: Cell<Option<(u64, u64)>>,
        gesture_drag: OnceCell<gtk::GestureDrag>,
        drag_start: Cell<f64>,
        drag_type: Cell<Option<DragType>>,
        cursor_type: Cell<CursorType>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Timeline {
        const NAME: &'static str = "Timeline";
        type Type = super::Timeline;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.set_css_name("timeline");
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }

        fn new() -> Self {
            Self {
                box_timeline_position: TemplateChild::default(),
                box_timeline_selection: TemplateChild::default(),
                box_wrapper: TemplateChild::default(),
                left_handle: TemplateChild::default(),
                right_handle: TemplateChild::default(),

                position: Cell::new(0),
                duration: Cell::new(0),
                range: Cell::new(Some((0, 0))),
                gesture_drag: OnceCell::new(),
                drag_start: Cell::new(0.),
                drag_type: Cell::new(None),
                cursor_type: Cell::new(CursorType::Normal),
            }
        }
    }

    impl ObjectImpl for Timeline {
        fn signals() -> &'static [Signal] {
            use once_cell::sync::Lazy;
            static SIGNALS: Lazy<[Signal; 3]> = Lazy::new(|| {
                [
                    Signal::builder("set-range")
                        .param_types([glib::Type::U64, glib::Type::U64])
                        .build(),
                    Signal::builder("set-position")
                        .param_types([glib::Type::U64])
                        .build(),
                    Signal::builder("moving").build(),
                ]
            });

            SIGNALS.as_ref()
        }

        fn constructed(&self) {
            let obj = self.obj();
            self.parent_constructed();

            // Invisible until we get duration.
            self.box_timeline_position.set_child_visible(false);
            self.box_timeline_selection.set_child_visible(false);

            // For some reason doesn't work from the .ui file.
            obj.set_overflow(gtk::Overflow::Hidden);

            // Set up the drag gesture.
            let gesture_drag = gtk::GestureDrag::new();
            gesture_drag.connect_drag_begin({
                let obj = obj.downgrade();
                move |_, x, y| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_drag_start(x, y);
                }
            });
            gesture_drag.connect_drag_update({
                let obj = obj.downgrade();
                move |_, offset_x, offset_y| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_drag_update(offset_x, offset_y);
                }
            });
            gesture_drag.connect_drag_end({
                let obj = obj.downgrade();
                move |_, _, _| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_drag_end();
                }
            });
            obj.add_controller(gesture_drag.clone());
            self.gesture_drag.set(gesture_drag).unwrap();

            let event_controller_motion = gtk::EventControllerMotion::new();
            event_controller_motion.connect_motion({
                let obj = obj.downgrade();
                move |_, x, y| {
                    let obj = obj.upgrade().unwrap();
                    let imp = obj.imp();
                    imp.on_motion(x, y);
                }
            });
            obj.add_controller(event_controller_motion);

            let event_controller_keyboard = gtk::EventControllerKey::new();
            event_controller_keyboard.connect_key_pressed(clone!(
                #[weak(rename_to = this)]
                self,
                #[upgrade_or]
                glib::Propagation::Stop,
                move |_, k, _, _| {
                    match k {
                        Key::Left => {
                            this.bring_start_back();
                            glib::Propagation::Stop
                        }
                        Key::Right => {
                            this.bring_start_forward();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            self.left_handle.add_controller(event_controller_keyboard);

            let event_controller_keyboard = gtk::EventControllerKey::new();
            event_controller_keyboard.connect_key_pressed(clone!(
                #[weak(rename_to = this)]
                self,
                #[upgrade_or]
                glib::Propagation::Stop,
                move |_, k, _, _| {
                    match k {
                        Key::Left => {
                            this.bring_end_back();
                            glib::Propagation::Stop
                        }
                        Key::Right => {
                            this.bring_end_forward();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            self.right_handle.add_controller(event_controller_keyboard);
        }

        fn dispose(&self) {
            let obj = self.obj();
            while let Some(child) = obj.first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for Timeline {
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let duration = self.duration.get();
            if duration == 0 {
                return;
            }

            let position = self.position.get();
            let x = ((position as f64 / duration as f64).clamp(0., 1.) * width as f64) as i32;
            let position_width = self
                .box_timeline_position
                .measure(gtk::Orientation::Horizontal, -1)
                .0;
            let position_height = self
                .box_timeline_position
                .measure(gtk::Orientation::Vertical, position_width)
                .0
                .max(height);

            self.box_timeline_position.size_allocate(
                &gtk::Allocation::new(x - position_width / 2, 0, position_width, position_height),
                baseline,
            );

            if let Some((start, end)) = self.range.get() {
                let duration = duration as f64;
                let x = ((start as f64 / duration).clamp(0., 1.) * width as f64) as i32;
                let x_end = ((end as f64 / duration).clamp(0., 1.) * width as f64) as i32;

                let selection_width = self
                    .box_timeline_selection
                    .measure(gtk::Orientation::Horizontal, -1)
                    .0
                    .max(x_end - x);
                let selection_height = self
                    .box_timeline_selection
                    .measure(gtk::Orientation::Vertical, selection_width)
                    .0
                    .max(height);

                self.box_timeline_selection.size_allocate(
                    &gtk::Allocation::new(x, 0, selection_width, selection_height),
                    baseline,
                );
            }
        }
    }

    impl Timeline {
        pub fn set_range(&self, range: Option<(u64, u64)>) {
            self.range.set(range);
            // if let Some((start, end)) = range {
            //     self.left_handle.set_tooltip_text(Some(&format_time(start)));
            //     self.right_handle.set_tooltip_text(Some(&format_time(end)));
            // }
            self.refresh();
        }

        pub fn refresh(&self) {
            let obj = self.obj();

            let duration = self.duration.get();
            if duration == 0 {
                self.box_timeline_position.set_child_visible(false);
                self.box_timeline_selection.set_child_visible(false);
                obj.queue_allocate();
                return;
            }

            self.box_timeline_position.set_child_visible(true);
            self.box_timeline_selection
                .set_child_visible(self.range.get().is_some());

            obj.queue_allocate();
        }

        fn on_drag_start(&self, x: f64, _y: f64) {
            self.obj().emit_by_name::<()>("moving", &[]);
            self.drag_start.set(x);
            self.drag_type.set(Some(DragType::Playback));

            if self.range.get().is_some() {
                let allocation = self
                    .box_timeline_selection
                    .compute_bounds(&self.box_timeline_selection.parent().unwrap())
                    .unwrap();
                let start = allocation.x() as f64;
                let end = (allocation.x() + allocation.width()) as f64;

                if (x - end).abs() <= TOLERANCE {
                    self.drag_type.set(Some(DragType::End));
                    self.drag_start.set(end);
                } else if (x - start).abs() <= TOLERANCE {
                    self.drag_type.set(Some(DragType::Start));
                    self.drag_start.set(start);
                }
            }

            self.on_drag_update(0., 0.);
        }

        fn on_drag_update(&self, offset_x: f64, _offset_y: f64) {
            let obj = self.obj();

            let x = self.drag_start.get() + offset_x;
            let width = obj.width() as f64;

            // Sanitize (this can get weird values when resizing the window while dragging).
            let x = x.clamp(0., width);
            let value = x / width;

            let duration = self.duration.get();

            if duration != 0 {
                let time = (duration as f64 * value) as u64;

                // Update the position for responsive seeking.
                self.set_position(time);
                obj.queue_allocate();

                let range = self.range.get();
                if range.is_none() {
                    return;
                }

                let (start, end) = range.unwrap();

                let (start, end) = match self.drag_type.get().unwrap() {
                    DragType::Start => {
                        if time <= end {
                            (time, end)
                        } else {
                            self.drag_type.set(Some(DragType::End));
                            (end, time)
                        }
                    }
                    DragType::End => {
                        if time >= start {
                            // self.set_position(start);
                            (start, time)
                        } else {
                            self.drag_type.set(Some(DragType::Start));
                            (time, start)
                        }
                    }
                    _ => return,
                };

                self.range.set(Some((start, end)));
                // self.left_handle.set_tooltip_text(Some(&format_time(start)));
                // self.right_handle.set_tooltip_text(Some(&format_time(end)));
                self.refresh();
            };
        }

        fn bring_start_forward(&self) {
            let (start, end) = self.range.get().unwrap();
            self.range.set(Some((
                (start + TIMELINE_KEYBOARD_MOVE as u64).min(end),
                end,
            )));
            let (start, end) = self.range.get().unwrap();
            self.set_position(start);
            self.obj().emit_by_name::<()>("set-range", &[&start, &end]);
            self.obj()
                .emit_by_name::<()>("set-position", &[&self.position.get()]);
            // self.left_handle.set_tooltip_text(Some(&format_time(start)));
            // self.right_handle.set_tooltip_text(Some(&format_time(end)));
        }

        fn bring_start_back(&self) {
            let (start, end) = self.range.get().unwrap();
            self.range.set(Some((
                (start as i64 - TIMELINE_KEYBOARD_MOVE).max(0) as u64,
                end,
            )));
            let (start, end) = self.range.get().unwrap();
            self.set_position(start);
            self.obj().emit_by_name::<()>("set-range", &[&start, &end]);
            self.obj()
                .emit_by_name::<()>("set-position", &[&self.position.get()]);
            // self.left_handle.set_tooltip_text(Some(&format_time(start)));
            // self.right_handle.set_tooltip_text(Some(&format_time(end)));
        }

        fn bring_end_forward(&self) {
            let (start, end) = self.range.get().unwrap();
            self.range.set(Some((
                start,
                (end + TIMELINE_KEYBOARD_MOVE as u64).min(self.duration.get()),
            )));
            let (start, end) = self.range.get().unwrap();
            self.set_position(end);
            self.obj().emit_by_name::<()>("set-range", &[&start, &end]);
            self.obj()
                .emit_by_name::<()>("set-position", &[&self.position.get()]);
        }

        fn bring_end_back(&self) {
            let (start, end) = self.range.get().unwrap();
            self.range.set(Some((
                start,
                (end as i64 - TIMELINE_KEYBOARD_MOVE).max(start as i64) as u64,
            )));
            let (start, end) = self.range.get().unwrap();
            self.set_position(end);
            self.obj().emit_by_name::<()>("set-range", &[&start, &end]);
            self.obj()
                .emit_by_name::<()>("set-position", &[&self.position.get()]);
            // self.left_handle.set_tooltip_text(Some(&format_time(start)));
            // self.right_handle.set_tooltip_text(Some(&format_time(end)));
        }

        fn on_drag_end(&self) {
            let (start, end) = self.range.get().unwrap();
            self.obj().emit_by_name::<()>("set-range", &[&start, &end]);
            self.obj()
                .emit_by_name::<()>("set-position", &[&self.position.get()]);
            // self.refresh();
            // self.left_handle.set_tooltip_text(Some(&format_time(start)));
            // self.right_handle.set_tooltip_text(Some(&format_time(end)));
            // self.set_position(start);
        }

        pub fn set_duration(&self, duration: u64) {
            self.duration.set(duration);
            self.refresh();
        }

        pub fn set_position(&self, position: u64) {
            let (start, end) = self.range.get().unwrap();
            let position = position.clamp(start, end);
            self.position.set(position);
            self.refresh();
        }

        fn on_motion(&self, x: f64, _y: f64) {
            let obj = self.obj();

            // Don't change the cursor while in drag.
            if self.gesture_drag.get().unwrap().is_active() {
                return;
            }

            let resizing_cursor = if self.range.get().is_some() {
                let allocation = self
                    .box_timeline_selection
                    .compute_bounds(&self.box_timeline_selection.parent().unwrap())
                    .unwrap();
                let start = allocation.x() as f64;
                let end = (allocation.x() + allocation.width()) as f64;

                (x - end).abs() <= TOLERANCE || (x - start).abs() <= TOLERANCE
            } else {
                false
            };

            let cursor_type = if resizing_cursor {
                CursorType::StartEnd
            } else {
                CursorType::Normal
            };

            if self.cursor_type.get() != cursor_type {
                let cursor = gdk::Cursor::from_name(cursor_type.gtk_cursor_name(), None).unwrap();
                obj.set_cursor(Some(&cursor));
                self.cursor_type.set(cursor_type);
            }
        }
    }
}

glib::wrapper! {
    pub struct Timeline(ObjectSubclass<imp::Timeline>)
        @extends gtk::Widget,
        @implements gtk::Root, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Timeline {
    pub fn set_range(&self, range: Option<(u64, u64)>) {
        self.imp().set_range(range);
    }

    pub fn set_duration(&self, duration: u64) {
        self.imp().set_duration(duration);
    }

    pub fn set_position(&self, position: u64) {
        self.imp().set_position(position);
    }
}

// fn format_time(time: u64) -> String {
//     dbg!(time);

//     let minutes = time / 60 / 1000;
//     let seconds = time / 60 % 60;
//     let cmseconds = time % 1000 / 10;

//     format!("{minutes}:{seconds}.{cmseconds}")
// }

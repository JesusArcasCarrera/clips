mod application;
#[rustfmt::skip]
mod config;
mod info;
mod orientation;
mod profiles;
mod widgets;
mod window;

use std::sync::OnceLock;

use gettextrs::{gettext, LocaleCategory};
use glib::ExitCode;
use gtk::{gio, glib};
use tokio::runtime::Runtime;

use self::application::App;
use self::config::{GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_FILE};

#[macro_export]
macro_rules! spawn {
    ($future:expr) => {
        let ctx = glib::MainContext::default();
        ctx.spawn_local($future);
    };
}

pub trait Listable {
    fn to_list(&self) -> gtk::StringList;
}

impl Listable for Vec<String> {
    fn to_list(&self) -> gtk::StringList {
        let list = gtk::StringList::new(&[]);
        for s in self {
            list.append(s);
        }
        list
    }
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("Setting up tokio runtime needs to succeed."))
}

fn main() -> ExitCode {
    // Avoid GTK Vulkan renderer crashes observed with gtk4paintablesink on some NVIDIA setups.
    if std::env::var_os("GSK_RENDERER").is_none() {
        std::env::set_var("GSK_RENDERER", "gl");
    }

    // Initialize logger
    pretty_env_logger::init();

    // Prepare i18n
    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    glib::set_application_name(&gettext("Clips"));

    let res = gio::Resource::load(RESOURCES_FILE).expect("Could not load gresource file");
    gio::resources_register(&res);

    let app = App::new();
    app.run()
}

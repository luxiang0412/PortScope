//! PortScope — application entry point.

fn main() -> gtk::glib::ExitCode {
    portscope_gtk::ui::run()
}

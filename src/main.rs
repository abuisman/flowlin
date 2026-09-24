use tracing_subscriber::EnvFilter;

fn main() -> gtk::glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,flowlin=info")))
        .with_writer(std::io::stderr)
        .init();
    flowlin::i18n::init();
    flowlin::app::run()
}

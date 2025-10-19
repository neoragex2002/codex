use clap::Parser;

#[ctor::ctor]
fn pre_main() {
    codex_process_hardening::pre_main_hardening();
}

pub fn main() -> anyhow::Result<()> {
    let args = codex_backend_proxy::Args::parse();
    codex_backend_proxy::run_main(args)
}

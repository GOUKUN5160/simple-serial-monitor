#![windows_subsystem = "windows"]

use simple_serial_monitor::{agent_api, config, manager, ui};

fn main() {
    let app_cfg = ui::load_initial_config();
    let log_dir = config::logs_dir_path();
    let mgr = manager::MonitorManager::new(log_dir, app_cfg.api_port);
    let actual_api_port = agent_api::start_agent_server(mgr.clone(), app_cfg.api_port);
    ui::run_gui(mgr, app_cfg, actual_api_port);
}

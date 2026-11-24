#ifndef SSM_MACOS_BRIDGE_H
#define SSM_MACOS_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Entry point implemented in Objective-C (bridge.m)
void ssm_macos_run_gui(const char *initial_config_json, int actual_api_port);

// Callbacks implemented in Rust (src/ui/macos/mod.rs)
void ssm_free_str(char *ptr);
char *ssm_get_ports_json(int refresh);
void ssm_attach_pane(const char *port, const char *serial_cfg_json, int auto_connect);
void ssm_acquire_pane_ref(const char *port);
void ssm_release_pane_ref(const char *port);
char *ssm_poll_pane_json(const char *port, uint64_t since_seq, size_t tail);
void ssm_connect(const char *port);
void ssm_disconnect(const char *port);
void ssm_autodetect(const char *port);
void ssm_kill_owner(const char *port);
void ssm_clear_logs(const char *port);
void ssm_update_serial_config(const char *port, const char *serial_cfg_json);
void ssm_send_text(const char *port, const char *text);
void ssm_set_lang(const char *lang_code);
void ssm_save_config_json(const char *config_json);
char *ssm_get_log_dir(void);
void ssm_shutdown_all(void);

#ifdef __cplusplus
}
#endif

#endif // SSM_MACOS_BRIDGE_H

mod commands;
mod state;

// Public so the integration tests in tests/ can drive a real runtime.
pub mod error;
pub mod model;
pub mod k8s;
pub mod runtime;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::new())
        .setup(|app| {
            // Pick a working runtime in the background so the window paints
            // immediately instead of blocking on daemon probes.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let state = handle.state::<AppState>();
                let selected = state.auto_select().await;
                use tauri::Emitter;
                let _ = handle.emit("runtime:ready", selected);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // runtime / system
            commands::list_runtimes,
            commands::current_runtime,
            commands::select_runtime,
            commands::system_summary,
            commands::ping_runtime,
            // containers
            commands::list_containers,
            commands::inspect_container,
            commands::start_container,
            commands::stop_container,
            commands::restart_container,
            commands::pause_container,
            commands::unpause_container,
            commands::remove_container,
            commands::create_container,
            commands::container_health,
            commands::container_logs,
            commands::container_stats,
            commands::list_container_services,
            commands::control_container_service,
            // exec
            commands::detect_shell,
            commands::exec_start,
            commands::exec_write,
            commands::exec_resize,
            commands::exec_stop,
            // images
            commands::list_images,
            commands::remove_image,
            commands::inspect_image,
            commands::image_history,
            commands::prune_images,
            commands::registry_logins,
            commands::registry_identity,
            // networks
            commands::list_networks,
            commands::create_network,
            commands::remove_network,
            commands::inspect_network,
            commands::connect_network,
            commands::disconnect_network,
            // volumes
            commands::list_volumes,
            commands::create_volume,
            commands::remove_volume,
            commands::inspect_volume,
            commands::prune_volumes,
            // compose
            commands::compose_services,
            commands::compose_up,
            commands::compose_down,
            commands::compose_restart,
            commands::compose_exec,
            commands::stop_compose_exec,
            // streaming
            commands::follow_logs,
            commands::stop_follow_logs,
            commands::stream_stats,
            commands::stop_stream_stats,
            commands::pull_image,
            commands::stop_pull,
            commands::tag_image,
            commands::push_image,
            commands::stop_push,
            commands::copy_image,
            commands::stop_copy_image,
            commands::stop_all_streams,
            // kubernetes
            commands::k8s_contexts,
            commands::k8s_probe_clusters,
            commands::k8s_current_context,
            commands::k8s_select_context,
            commands::k8s_list_namespaces,
            commands::k8s_list_pods,
            commands::k8s_list_deployments,
            commands::k8s_list_services,
            commands::k8s_list_nodes,
            commands::k8s_inspect_pod,
            commands::k8s_delete_pod,
            commands::k8s_pod_logs,
            commands::k8s_follow_pod_logs,
            commands::k8s_stop_pod_logs,
            commands::k8s_apply_manifest,
            commands::k8s_delete_manifest,
            commands::k8s_ensure_namespace,
            commands::k8s_generate_from_container,
            commands::k8s_generate_from_compose,
            // activity
            commands::activity_log,
            commands::clear_activity_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

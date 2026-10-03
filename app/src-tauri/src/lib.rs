mod analytics_commands;
mod answer_validator;
mod api_key_store;
mod audit_commands;
mod chat_history;
mod context_commands;
mod database_commands;
mod diagnostic_commands;
mod doc_gen_commands;
mod document_upload_commands;
mod enhanced_rag_commands;
mod file_watcher;
mod history_commands;
mod image_upload_commands;
mod llm_bootstrap;
mod llm_commands;
mod llm_response;
mod mcp;
mod mcp_commands;
mod rag_commands;
mod search_history;
mod search_models_commands;
mod smart_templates;
mod source_viewer_commands;
mod space_commands;
mod space_manager;
mod storage_commands;
mod system_commands;
mod template_commands;
mod window_commands;

// Unified chat system modules
mod agent_session_commands;
mod agent_tools;
mod calendar_commands;
mod conversation_commands;
mod event_emitter;

use tauri::Manager;

use analytics_commands::AnalyticsState;
use chat_history::ChatHistoryManager;
use context_commands::ContextState;
use enhanced_rag_commands::IndexingState;
use llm_commands::{ApiKeys, LLMState};
use mcp_commands::MCPState;
use rag_commands::{AppPaths, RagState};
use search_history::SearchHistoryManager;
use shodh_rag::llm::LLMConfig;
use space_manager::SpaceManager;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use template_commands::TemplateStore;
use tokio::sync::RwLock as AsyncRwLock;
use uuid::Uuid;

/// Resolve the directory holding the search models (E5 + reranker).
///
/// 1. `MODEL_PATH` environment variable: always used when set (explicit
///    override, also the install target).
///
/// Otherwise the first directory that already contains `multilingual-e5-base/`:
/// 2. Adjacent to the executable: `<exe_dir>/models/`
/// 3. Two levels up from the executable: `<exe_dir>/../../models/`
/// 4. Inside app data: `<app_data_dir>/models/`
///
/// When none has the models yet (first run), this is where
/// `install_search_models` downloads them:
/// - debug builds: the checkout's `models/` (from `CARGO_MANIFEST_DIR`);
/// - release builds: `<app_data_dir>/models/` — never a build-machine path.
fn resolve_model_dir(app_data_dir: &std::path::Path) -> PathBuf {
    let e5_subdir = shodh_rag::embeddings::model_store::E5_DIR;

    // 1. Explicit env var
    if let Some(env_path) = std::env::var_os("MODEL_PATH").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(env_path);
        tracing::info!("Model dir from MODEL_PATH env var: {:?}", p);
        return p;
    }

    // 2. Adjacent to executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("models");
            if candidate.join(e5_subdir).exists() {
                tracing::info!("Model dir adjacent to exe: {:?}", candidate);
                return candidate;
            }

            // 3. Two levels up (dev layout: target/debug/ → project root)
            if let Some(grandparent) = exe_dir.parent().and_then(|p| p.parent()) {
                let candidate = grandparent.join("models");
                if candidate.join(e5_subdir).exists() {
                    tracing::info!("Model dir from dev layout: {:?}", candidate);
                    return candidate;
                }
            }
        }
    }

    // 4. App data directory
    let candidate = app_data_dir.join("models");
    if candidate.join(e5_subdir).exists() {
        tracing::info!("Model dir from app data: {:?}", candidate);
        return candidate;
    }

    // Not installed anywhere yet: choose the install location.
    if cfg!(debug_assertions) {
        if let Some(repo_root) = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
        {
            let dev = repo_root.join("models");
            tracing::info!("Search models not installed; dev install dir: {:?}", dev);
            return dev;
        }
    }
    tracing::info!("Search models not installed; install dir: {:?}", candidate);
    candidate
}

/// Report a setup failure that leaves the app unable to start.
fn setup_error(context: &str, error: impl std::fmt::Display) -> Box<dyn std::error::Error> {
    let message = format!("{context}: {error}");
    tracing::error!("{}", message);
    message.into()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing subscriber so tracing::info!/debug!/warn!/error! produce output
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // Get app data directory for persistent storage. Without it
            // nothing can be stored, so this is the one fatal setup error.
            let app_data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| setup_error("Failed to resolve the app data directory", e))?;
            std::fs::create_dir_all(&app_data_dir)
                .map_err(|e| setup_error("Failed to create the app data directory", e))?;

            tracing::info!("App data directory: {:?}", app_data_dir);

            // Audit log first, so every later event can be recorded.
            let audit_state = audit_commands::AuditState::open(&app_data_dir);
            audit_state.spawn_retention();
            app.manage(audit_state);

            // Resolve model directory with multi-tier fallback for portability
            let model_dir = resolve_model_dir(&app_data_dir);
            tracing::info!("Model directory: {:?}", model_dir);
            app.manage(search_models_commands::SearchModelsState::new(
                model_dir.clone(),
            ));

            // Initialize SpaceManager with persistent storage
            let space_manager = SpaceManager::with_data_dir(app_data_dir.clone());

            // Create app paths
            let app_paths = AppPaths {
                data_dir: app_data_dir.clone(),
                db_path: app_data_dir.join("kalki_data"),
            };

            // Initialize LLMState FIRST so RagState can reference its manager
            let shared_llm_manager = Arc::new(AsyncRwLock::new(None));

            app.manage(LLMState {
                manager: shared_llm_manager.clone(),
                config: Arc::new(Mutex::new(LLMConfig::default())),
                api_keys: Arc::new(Mutex::new(ApiKeys::default())),
                custom_model_path: Arc::new(Mutex::new(None)),
            });

            // Load provider API keys saved in the OS credential store, then
            // apply the opt-in SHODH_LLM_PROVIDER / SHODH_LLM_MODEL environment
            // configuration. Order matters: environment keys override stored ones.
            let llm_bootstrap_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let llm_state = llm_bootstrap_handle.state::<LLMState>();
                match tokio::task::spawn_blocking(api_key_store::load_all).await {
                    Ok(stored) => {
                        let merged = llm_state
                            .api_keys
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .merge_missing(stored);
                        if merged > 0 {
                            tracing::info!(
                                "Loaded {} provider API key(s) from the OS credential store",
                                merged
                            );
                        }
                    }
                    Err(e) => tracing::warn!("Loading stored API keys failed: {}", e),
                }
                match llm_bootstrap::configure_from_environment(&llm_state).await {
                    Ok(Some(description)) => {
                        tracing::info!("LLM configured from environment: {}", description)
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("LLM environment configuration failed: {}", e),
                }
            });

            // Initialize the RAG engine. Without the search models (first
            // run) it starts in a "needs setup" state; the frontend offers
            // `install_search_models`, which attaches them without a restart.
            let mut rag_config = shodh_rag::config::RAGConfig::default();
            rag_config.embedding.model_dir = model_dir.clone();
            rag_config.embedding.use_e5 = true;
            // The vector index dimension must match the model that will be
            // attached: the installed E5 variant, else the pinned E5 base (768).
            rag_config.embedding.dimension =
                shodh_rag::embeddings::e5::E5Config::auto_detect(&model_dir)
                    .map(|c| c.dimension)
                    .unwrap_or(768);
            rag_config.data_dir = app_data_dir.clone();
            let default_rag = tauri::async_runtime::block_on(
                shodh_rag::comprehensive_system::ComprehensiveRAG::new(rag_config),
            )
            .map_err(|e| setup_error("Failed to open the document index", format!("{e:#}")))?;
            if !default_rag.has_search_models() {
                tracing::warn!("Search models are not installed; search needs first-run setup");
            }

            app.manage(RagState {
                rag: Arc::new(AsyncRwLock::new(default_rag)),
                notes: Mutex::new(Vec::new()),
                space_manager: Mutex::new(space_manager),
                conversation_manager: Arc::new(AsyncRwLock::new(None)),
                memory_system: Arc::new(AsyncRwLock::new(None)),
                app_paths,
                rag_initialized: Arc::new(AsyncRwLock::new(false)),
                initialization_lock: Arc::new(tokio::sync::Mutex::new(())),
            });

            app.manage(IndexingState::default());
            app.manage(agent_session_commands::AgentSessions::default());
            let analytics_path = app_data_dir.join("analytics.json");
            app.manage(AnalyticsState::load_or_default(&analytics_path));
            app.manage(TemplateStore::default());

            // Initialize MCP (Model Context Protocol) state
            let mcp_config_dir = app_data_dir.join("mcp");
            if let Err(e) = std::fs::create_dir_all(&mcp_config_dir) {
                tracing::error!(
                    "Failed to create MCP config directory {:?}: {}; MCP settings will not be saved",
                    mcp_config_dir,
                    e
                );
            }
            let mcp_manager = mcp::MCPManager::new();
            let mcp_registry = mcp::registry::MCPRegistry::new(mcp_config_dir);
            app.manage(MCPState {
                manager: Arc::new(AsyncRwLock::new(mcp_manager)),
                registry: Arc::new(AsyncRwLock::new(mcp_registry)),
            });

            // Initialize context accumulator with unique session ID
            let session_id = Uuid::new_v4().to_string();
            app.manage(ContextState::new(session_id));

            // Initialize search and chat history managers
            let search_history_manager = SearchHistoryManager::new(&app_data_dir);
            app.manage(Arc::new(Mutex::new(search_history_manager)));

            let chat_history_manager = ChatHistoryManager::new(&app_data_dir);
            app.manage(Arc::new(Mutex::new(chat_history_manager)));

            // Initialize conversation manager and memory system with app data directory
            let memory_store_path = app_data_dir.join("memory_store");

            let rag_state = app.state::<RagState>();
            let conversation_manager_arc = rag_state.conversation_manager.clone();
            let memory_system_arc_state = rag_state.memory_system.clone();

            tauri::async_runtime::spawn(async move {
                let mut memory_config = shodh_rag::memory::MemoryConfig::default();
                memory_config.storage_path = memory_store_path;

                match shodh_rag::memory::MemorySystem::new(memory_config) {
                    Ok(memory_system) => {
                        let memory_system_shared = Arc::new(AsyncRwLock::new(memory_system));
                        *memory_system_arc_state.write().await = Some(memory_system_shared.clone());
                        tracing::info!("Memory system initialized successfully");

                        match shodh_rag::agent::ConversationManager::new_with_memory(
                            memory_system_shared.clone(),
                        ) {
                            Ok(manager) => {
                                *conversation_manager_arc.write().await = Some(manager);
                                tracing::info!("Conversation manager initialized successfully");
                            }
                            Err(e) => {
                                tracing::error!("Failed to initialize conversation manager: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to initialize memory system: {}", e);
                    }
                }
            });

            // Re-index existing calendar data into RAG engine (best-effort, background)
            {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    // Brief delay to let RAG engine finish initializing
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let ready = app_handle
                        .state::<RagState>()
                        .rag
                        .read()
                        .await
                        .has_search_models();
                    // Without search models this would fail per item; the
                    // install command re-runs it once they are attached.
                    if ready {
                        calendar_commands::reindex_all_calendar_data(&app_handle).await;
                    }
                });
            }

            // Start with the configured (default: disabled) LLM mode unless the
            // environment bootstrap above already installed a manager.
            let llm_state = app.state::<LLMState>();
            let manager_clone = llm_state.manager.clone();
            let config_clone = llm_state.config.clone();

            tauri::async_runtime::spawn(async move {
                let config = config_clone
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let mut llm_manager = shodh_rag::llm::LLMManager::new(config);
                if let Err(e) = llm_manager.initialize().await {
                    tracing::error!("Failed to initialize LLM manager: {}", e);
                    return;
                }
                let mut slot = manager_clone.write().await;
                if slot.is_none() {
                    *slot = Some(llm_manager);
                    tracing::info!("LLM manager initialized");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // RAG commands
            rag_commands::initialize_rag,
            rag_commands::check_initialization_status,
            rag_commands::search_documents,
            rag_commands::add_document,
            rag_commands::upload_file,
            rag_commands::get_statistics,
            rag_commands::clear_all_data,
            rag_commands::delete_folder_source,
            rag_commands::add_test_documents,
            rag_commands::get_all_documents,
            rag_commands::list_space_documents,
            rag_commands::get_notes,
            rag_commands::save_note,
            rag_commands::update_note,
            rag_commands::delete_note,
            rag_commands::add_note_to_rag,
            rag_commands::remove_note_from_rag,
            rag_commands::link_folder,
            rag_commands::get_folder_stats,
            rag_commands::get_source_files,
            // First-run search model setup
            search_models_commands::search_models_status,
            search_models_commands::install_search_models,
            // Enhanced RAG commands
            enhanced_rag_commands::preview_folder,
            enhanced_rag_commands::link_folder_enhanced,
            enhanced_rag_commands::index_single_file,
            enhanced_rag_commands::test_indexing,
            enhanced_rag_commands::pause_indexing,
            enhanced_rag_commands::resume_indexing,
            enhanced_rag_commands::cancel_indexing,
            enhanced_rag_commands::check_path_type,
            // Window commands
            window_commands::create_floating_widget,
            window_commands::show_main_window,
            window_commands::watch_folder,
            window_commands::unwatch_folder,
            window_commands::watch_global_folder,
            window_commands::scan_global_folder,
            // Analytics commands
            rag_commands::get_daily_brief,
            rag_commands::get_knowledge_map,
            // Document access commands
            rag_commands::open_original_document,
            rag_commands::open_file_at_location,
            rag_commands::read_original_file,
            rag_commands::get_document_metadata,
            rag_commands::get_document_full_text,
            // Citation tracking commands
            rag_commands::jump_to_source,
            // Smart Templates commands
            template_commands::extract_template,
            template_commands::generate_from_template,
            template_commands::list_templates,
            template_commands::get_template,
            template_commands::delete_template,
            template_commands::update_template,
            template_commands::preview_template,
            // File watcher commands
            file_watcher::start_watching_folder,
            file_watcher::stop_watching_folder,
            file_watcher::get_watched_folders,
            // LLM commands
            llm_commands::switch_llm_mode,
            llm_commands::llm_generate,
            llm_commands::llm_generate_stream,
            llm_commands::llm_generate_stream_with_rag,
            llm_commands::get_llm_info,
            llm_commands::set_api_key,
            llm_commands::delete_api_key,
            llm_commands::get_configured_providers,
            llm_commands::update_llm_config,
            llm_commands::browse_model_file,
            llm_commands::set_custom_model_path,
            llm_commands::get_custom_model_path,
            llm_commands::test_llm_inference,
            // Space commands
            space_commands::create_space,
            space_commands::get_spaces,
            space_commands::add_document_to_space,
            space_commands::search_in_space,
            space_commands::search_global,
            space_commands::delete_space_with_docs,
            space_commands::get_space_documents,
            space_commands::remove_document,
            space_commands::set_space_system_prompt,
            space_commands::get_space_system_prompt,
            // History commands
            history_commands::add_search_history,
            history_commands::get_search_history,
            history_commands::get_search_suggestions,
            history_commands::clear_search_history,
            history_commands::add_chat_message,
            history_commands::get_chat_history,
            history_commands::clear_chat_history,
            history_commands::get_chat_sessions_summary,
            history_commands::export_chat_history,
            history_commands::search_with_history,
            // Graph commands
            // Document generation commands
            doc_gen_commands::generate_document,
            doc_gen_commands::generate_from_rag,
            doc_gen_commands::generate_document_stream,
            doc_gen_commands::get_available_formats,
            doc_gen_commands::get_available_templates,
            doc_gen_commands::generate_document_preview,
            doc_gen_commands::get_source_documents,
            doc_gen_commands::get_comparable_documents,
            // Database management commands
            database_commands::reset_database,
            database_commands::clear_all_documents,
            database_commands::delete_space_permanently,
            database_commands::get_database_stats,
            database_commands::list_indexed_sources,
            database_commands::cleanup_orphaned_documents,
            database_commands::save_backup_file,
            database_commands::read_backup_file,
            database_commands::restore_space_from_backup,
            database_commands::list_backup_files,
            database_commands::update_space_metadata,
            // Diagnostic commands
            diagnostic_commands::get_index_diagnostics,
            diagnostic_commands::get_document_content,
            diagnostic_commands::debug_rag_state,
            // Analytics commands
            analytics_commands::get_dashboard_data,
            analytics_commands::track_query,
            analytics_commands::track_query_error,
            analytics_commands::track_indexing,
            analytics_commands::get_performance_metrics,
            analytics_commands::get_usage_metrics,
            analytics_commands::get_quality_metrics,
            // Storage commands
            storage_commands::get_storage_stats,
            storage_commands::get_space_documents_detailed,
            storage_commands::delete_documents_batch,
            storage_commands::clear_space_documents,
            storage_commands::optimize_storage,
            storage_commands::create_backup,
            storage_commands::restore_backup,
            // Context accumulator commands
            context_commands::update_context,
            context_commands::track_user_message,
            context_commands::track_assistant_message,
            context_commands::track_search,
            context_commands::track_search_refinement,
            context_commands::track_document_view,
            context_commands::track_filter,
            context_commands::get_context_summary,
            context_commands::build_llm_context,
            context_commands::get_full_context,
            context_commands::clear_context,
            context_commands::start_task,
            context_commands::save_session_to_memory,
            context_commands::restore_session_from_memory,
            // Document commands (in rag_commands.rs)
            rag_commands::get_document_preview,
            source_viewer_commands::get_source_file_info,
            source_viewer_commands::read_source_bytes,
            source_viewer_commands::read_source_text,
            source_viewer_commands::read_source_table,
            rag_commands::parse_llm_response,
            // Image upload commands
            image_upload_commands::read_clipboard_image,
            image_upload_commands::process_image_from_base64,
            image_upload_commands::process_image_from_file,
            image_upload_commands::search_images,
            // Form export commands
            image_upload_commands::export_form_html,
            image_upload_commands::export_form_json,
            // System actions (OS integration)
            system_commands::execute_file_action,
            system_commands::execute_command_action,
            system_commands::open_file_manager,
            system_commands::get_system_information,
            system_commands::get_running_processes,
            // MCP (Model Context Protocol) commands
            mcp_commands::mcp_connect_server,
            mcp_commands::mcp_disconnect_server,
            mcp_commands::mcp_list_tools,
            mcp_commands::mcp_search_tools,
            mcp_commands::mcp_call_tool,
            mcp_commands::mcp_list_servers,
            mcp_commands::mcp_upsert_server,
            mcp_commands::mcp_remove_server,
            mcp_commands::mcp_update_server_env,
            // Document Upload commands
            document_upload_commands::upload_document_file,
            document_upload_commands::save_temp_file,
            // Agent sessions (omp harness)
            agent_session_commands::agent_start,
            agent_session_commands::agent_send,
            agent_session_commands::agent_steer,
            agent_session_commands::agent_abort,
            agent_session_commands::agent_approve,
            agent_session_commands::agent_install_runtime,
            audit_commands::audit_query,
            audit_commands::audit_verify,
            audit_commands::audit_export,
            audit_commands::audit_set_retention_days,
            audit_commands::audit_stats,
            agent_session_commands::agent_runtime_status,
            // Conversation persistence commands
            conversation_commands::load_conversations,
            conversation_commands::save_conversation,
            conversation_commands::delete_conversation,
            conversation_commands::rename_conversation,
            conversation_commands::pin_conversation,
            // Calendar/Todo commands
            calendar_commands::load_tasks,
            calendar_commands::create_task,
            calendar_commands::update_task,
            calendar_commands::delete_task,
            calendar_commands::add_subtask,
            calendar_commands::toggle_subtask,
            calendar_commands::delete_subtask,
            calendar_commands::load_events,
            calendar_commands::create_event,
            calendar_commands::update_event,
            calendar_commands::delete_event,
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("Shodh could not start: {}", e);
            eprintln!("Shodh could not start: {e}");
            std::process::exit(1);
        }
    };
    app.run(|app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Stop every omp sidecar before the process exits.
            let sessions = app_handle.state::<agent_session_commands::AgentSessions>();
            tauri::async_runtime::block_on(sessions.shutdown_all());
        }
    });
}

mod rag_commands;
mod window_commands;
mod file_watcher;
mod llm_commands;
mod llm_response;
mod space_commands;
mod enhanced_rag_commands;
mod space_manager;
mod search_history;
mod chat_history;
mod history_commands;
mod graph_commands;
mod doc_gen_commands;
mod database_commands;
mod diagnostic_commands;
mod analytics_commands;
mod storage_commands;
mod smart_templates;
mod template_commands;
mod query_rewriter;
mod retrieval_commands;
mod context_commands;
mod whatsapp_bot;
mod whatsapp_commands;
mod whatsapp_http_server;
mod telegram_http_server;
mod telegram_bot_commands;
mod discord_http_server;
mod discord_bot_commands;
mod google_drive_commands;
mod image_upload_commands;
mod system_commands;
mod mcp;
mod mcp_commands;
mod mcp_bridge;
mod document_upload_commands;

// Unified chat system modules
mod chat_engine;
mod artifact_store;
mod unified_chat_commands;
mod conversation_commands;
mod agent_commands;
mod calendar_commands;

use tauri::Manager;

use rag_commands::{RagState, AppPaths};
use context_commands::ContextState;
use uuid::Uuid;
use enhanced_rag_commands::IndexingState;
use llm_commands::{LLMState, ApiKeys};
use space_manager::SpaceManager;
use search_history::SearchHistoryManager;
use chat_history::ChatHistoryManager;
use analytics_commands::AnalyticsState;
use template_commands::TemplateStore;
use whatsapp_commands::WhatsAppBotState;
use telegram_bot_commands::TelegramBotState;
use discord_bot_commands::DiscordBotState;
use google_drive_commands::GoogleDriveState;
use mcp_commands::MCPState;
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock as AsyncRwLock;
use shodh_rag::llm::{LLMConfig, ModelManager};
use std::path::PathBuf;

/// Resolve the models directory with multi-tier fallback for portability.
///
/// Search order:
/// 1. `MODEL_PATH` environment variable (explicit override)
/// 2. Adjacent to executable: `<exe_dir>/models/`
/// 3. Two levels up from executable: `<exe_dir>/../../models/` (dev layout)
/// 4. Inside app data: `<app_data_dir>/models/`
/// 5. Compile-time source tree path (only works on the build machine)
fn resolve_model_dir(app_data_dir: &std::path::Path) -> PathBuf {
    let e5_subdir = "multilingual-e5-base";

    // 1. Explicit env var
    if let Ok(env_path) = std::env::var("MODEL_PATH") {
        let p = PathBuf::from(&env_path);
        if p.join(e5_subdir).exists() {
            tracing::info!("Model dir from MODEL_PATH env var: {:?}", p);
            return p;
        }
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

    // 5. Compile-time fallback (works on build machine only)
    let compile_time = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("models"))
        .unwrap_or_else(|| PathBuf::from("models"));

    tracing::info!("Model dir from compile-time path: {:?}", compile_time);
    compile_time
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing subscriber so tracing::info!/debug!/warn!/error! produce output
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
        )
        .with_target(false)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            // Get app data directory for persistent storage
            let app_data_dir = app.path().app_data_dir()
                .expect("Failed to get app data directory");

            // Create app data directory if it doesn't exist
            if !app_data_dir.exists() {
                std::fs::create_dir_all(&app_data_dir)
                    .expect("Failed to create app data directory");
            }

            tracing::info!("App data directory: {:?}", app_data_dir);

            // Resolve model directory with multi-tier fallback for portability
            let model_dir = resolve_model_dir(&app_data_dir);
            tracing::info!("Model directory: {:?}", model_dir);
            tracing::info!("E5 model exists: {}", model_dir.join("multilingual-e5-base").exists());

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
                model_manager: Arc::new(ModelManager::new(model_dir.clone())),
                config: Arc::new(Mutex::new(LLMConfig::default())),
                api_keys: Arc::new(Mutex::new(ApiKeys::default())),
                custom_model_path: Arc::new(Mutex::new(None)),
                custom_tokenizer_path: Arc::new(Mutex::new(None)),
            });

            // Initialize RagState with explicit model path configuration
            let mut rag_config = shodh_rag::config::RAGConfig::default();
            rag_config.embedding.model_dir = model_dir.clone();
            rag_config.embedding.use_e5 = model_dir.join("multilingual-e5-base").exists();
            if rag_config.embedding.use_e5 {
                rag_config.embedding.dimension = 768;
            }
            rag_config.data_dir = app_data_dir.clone();
            let default_rag = tauri::async_runtime::block_on(
                shodh_rag::comprehensive_system::ComprehensiveRAG::new(rag_config)
            ).expect("Failed to create default RAG instance");

            app.manage(RagState {
                rag: Arc::new(AsyncRwLock::new(default_rag)),
                notes: Mutex::new(Vec::new()),
                space_manager: Mutex::new(space_manager),
                conversation_manager: Arc::new(AsyncRwLock::new(None)),
                memory_system: Arc::new(AsyncRwLock::new(None)),
                personal_assistant: Arc::new(AsyncRwLock::new(None)),
                app_paths,
                rag_initialized: Arc::new(AsyncRwLock::new(false)),
                initialization_lock: Arc::new(tokio::sync::Mutex::new(())),

                // Unified chat system
                artifact_store: Arc::new(AsyncRwLock::new(artifact_store::ArtifactStore::new())),
                conversation_id: Arc::new(AsyncRwLock::new(None)),
                agent_system: Arc::new(AsyncRwLock::new(None)),
                llm_manager: shared_llm_manager.clone(),
            });

            app.manage(IndexingState::default());
            let analytics_path = app_data_dir.join("analytics.json");
            app.manage(AnalyticsState::load_or_default(&analytics_path));
            app.manage(TemplateStore::default());
            app.manage(WhatsAppBotState::default());
            app.manage(TelegramBotState {
                process: Mutex::new(None),
                server_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            app.manage(DiscordBotState {
                process: Mutex::new(None),
                server_started: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            });
            app.manage(Arc::new(GoogleDriveState::new()));

            // Initialize MCP (Model Context Protocol) state
            let mcp_config_dir = app_data_dir.join("mcp");
            if !mcp_config_dir.exists() {
                std::fs::create_dir_all(&mcp_config_dir)
                    .expect("Failed to create MCP config directory");
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
            let search_history_manager = SearchHistoryManager::new(&app.app_handle())
                .expect("Failed to initialize search history manager");
            app.manage(Arc::new(Mutex::new(search_history_manager)));

            let chat_history_manager = ChatHistoryManager::new(&app.app_handle())
                .expect("Failed to initialize chat history manager");
            app.manage(Arc::new(Mutex::new(chat_history_manager)));

            // Initialize conversation manager and memory system with app data directory
            let app_dir = app.path().app_data_dir()
                .expect("Failed to get app data directory");
            let memory_store_path = app_dir.join("memory_store");

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

                        match shodh_rag::agent::ConversationManager::new_with_memory(memory_system_shared.clone()) {
                            Ok(manager) => {
                                *conversation_manager_arc.write().await = Some(manager);
                                tracing::info!("Conversation manager initialized successfully");
                            },
                            Err(e) => {
                                tracing::error!("Failed to initialize conversation manager: {}", e);
                            }
                        }
                    },
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
                    calendar_commands::reindex_all_calendar_data(&app_handle).await;
                });
            }

            // Bot HTTP servers (WhatsApp, Telegram, Discord) are now started lazily
            // when the user explicitly starts a bot via whatsapp_initialize / start_telegram_bot / start_discord_bot.
            // This avoids binding ports 3456/3458/3459 unconditionally on app launch.

            // Initialize LLM manager on startup
            let llm_state = app.state::<LLMState>();
            let manager_clone = llm_state.manager.clone();
            let config_clone = llm_state.config.clone();

            tauri::async_runtime::spawn(async move {
                let config = config_clone.lock().unwrap_or_else(|e| e.into_inner()).clone();

                let model_dir = if cfg!(debug_assertions) {
                    let exe_dir = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| std::env::current_dir().unwrap());
                    exe_dir.join("../../../../models")
                } else {
                    let exe_dir = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                        .unwrap_or_else(|| std::env::current_dir().unwrap());
                    exe_dir.join("models")
                };

                let mut llm_manager = shodh_rag::llm::LLMManager::new_with_cache_dir(config, model_dir);
                if let Err(e) = llm_manager.initialize().await {
                    tracing::error!("Failed to initialize LLM manager: {}", e);
                } else {
                    *manager_clone.write().await = Some(llm_manager);
                    tracing::info!("LLM manager initialized successfully");
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
            llm_commands::initialize_llm,
            llm_commands::switch_llm_mode,
            llm_commands::llm_generate,
            llm_commands::llm_generate_stream,
            llm_commands::llm_generate_stream_with_rag,
            llm_commands::get_llm_info,
            llm_commands::set_api_key,
            llm_commands::is_model_cached,
            llm_commands::download_model,
            llm_commands::get_model_cache_info,
            llm_commands::delete_cached_model,
            llm_commands::update_llm_config,
            llm_commands::browse_model_file,
            llm_commands::browse_tokenizer_file,
            llm_commands::set_custom_model_path,
            llm_commands::set_custom_tokenizer_path,
            llm_commands::initialize_llm_with_custom_path,
            llm_commands::get_custom_model_path,
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
            graph_commands::get_knowledge_graph,
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
            // Query rewriting command
            query_rewriter::search_with_query_rewriting,
            // Retrieval decision commands
            retrieval_commands::analyze_query,
            retrieval_commands::get_corpus_stats,
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
            rag_commands::parse_llm_response,
            // WhatsApp Bot commands
            whatsapp_commands::whatsapp_initialize,
            whatsapp_commands::whatsapp_add_contact,
            whatsapp_commands::whatsapp_update_contact_preferences,
            whatsapp_commands::whatsapp_assign_space,
            whatsapp_commands::whatsapp_process_message,
            whatsapp_commands::whatsapp_list_contacts,
            whatsapp_commands::whatsapp_get_stats,
            whatsapp_commands::whatsapp_set_active,
            whatsapp_commands::whatsapp_stop,
            whatsapp_commands::whatsapp_remove_contact,
            whatsapp_commands::whatsapp_get_conversation,
            whatsapp_commands::whatsapp_test_message,
            // Telegram Bot commands
            telegram_bot_commands::start_telegram_bot,
            telegram_bot_commands::stop_telegram_bot,
            telegram_bot_commands::check_telegram_bot_status,
            // Discord Bot commands
            discord_bot_commands::start_discord_bot,
            discord_bot_commands::stop_discord_bot,
            discord_bot_commands::check_discord_bot_status,
            // Google Drive integration commands
            google_drive_commands::init_google_drive_oauth,
            google_drive_commands::exchange_google_drive_code,
            google_drive_commands::list_google_drive_files,
            google_drive_commands::download_google_drive_file,
            google_drive_commands::configure_folder_sync,
            google_drive_commands::sync_google_drive_folder,
            google_drive_commands::get_google_drive_sync_status,
            google_drive_commands::is_google_drive_authenticated,
            google_drive_commands::disconnect_google_drive,
            // Image upload commands
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
            // Unified Chat System commands
            unified_chat_commands::unified_chat,
            unified_chat_commands::apply_artifact_to_file,
            unified_chat_commands::update_artifact,
            unified_chat_commands::get_artifact_history,
            unified_chat_commands::get_conversation_artifacts,
            // Conversation persistence commands
            conversation_commands::load_conversations,
            conversation_commands::save_conversation,
            conversation_commands::delete_conversation,
            conversation_commands::rename_conversation,
            conversation_commands::pin_conversation,
            // Agent commands
            agent_commands::get_agent_dashboard,
            agent_commands::get_active_executions,
            agent_commands::toggle_agent,
            agent_commands::create_agent,
            agent_commands::update_agent,
            agent_commands::delete_agent,
            agent_commands::get_agent,
            agent_commands::list_agents,
            agent_commands::execute_agent,
            // Crew commands
            agent_commands::create_crew,
            agent_commands::get_crew,
            agent_commands::list_crews,
            agent_commands::delete_crew,
            agent_commands::execute_crew,
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
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

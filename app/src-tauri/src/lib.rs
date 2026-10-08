mod analytics_commands;
mod answer_check_commands;
mod api_key_store;
mod app_settings;
mod audit_commands;
mod background;
mod chat_history;
mod connect_commands;
mod database_commands;
mod diagnostic_commands;
mod doc_gen_commands;
mod document_upload_commands;
mod enhanced_rag_commands;
mod file_watcher;
mod graph_commands;
mod history_commands;
mod image_upload_commands;
mod inbox_commands;
mod library_commands;
mod llm_bootstrap;
mod llm_commands;
mod llm_response;
mod mcp;
mod mcp_commands;
mod memory_commands;
mod memory_learn;
mod model_picker_commands;
mod pdf_export;
mod profile;
mod rag_commands;
mod reminders;
mod research_commands;
mod search_history;
mod search_models_commands;
mod smart_templates;
mod source_viewer_commands;
mod space_commands;
mod space_manager;
mod storage_commands;
mod system_commands;
mod table_model_commands;
mod template_commands;
mod visual_commands;
mod window_commands;
mod workspace_commands;

// Unified chat system modules
mod agent_coverage;
mod agent_session_commands;
mod agent_tools;
mod calendar_commands;
mod calendar_store;
mod conversation_commands;
mod event_emitter;

use tauri::Manager;

use analytics_commands::AnalyticsState;
use chat_history::ChatHistoryManager;
use enhanced_rag_commands::IndexingState;
use llm_commands::{ApiKeys, LLMState};
use rag_commands::{AppPaths, RagState};
use search_history::SearchHistoryManager;
use shodh_rag::llm::LLMConfig;
use space_manager::SpaceManager;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use template_commands::TemplateStore;
use tokio::sync::RwLock as AsyncRwLock;

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

    // The data folder decides everything below, so it is fixed first.
    let profile = match profile::init() {
        Ok(profile) => profile,
        Err(e) => {
            tracing::error!("{}", e);
            std::process::exit(2);
        }
    };
    if let Some(dir) = profile.data_dir() {
        tracing::info!("Separate profile from {}: {:?}", profile::DATA_DIR_ENV, dir);
    }

    let mut builder = tauri::Builder::default();
    if profile.uses_single_instance() {
        // First, so a second launch exits before it opens any data.
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            background::on_second_launch(app, &args);
        }));
    }
    let app = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![background::BACKGROUND_FLAG]),
        ))
        .on_window_event(background::on_window_event)
        .setup(|app| {
            // The main window is created here rather than from the config, so
            // its WebView storage follows the profile.
            for config in app.config().app.windows.iter() {
                profile::webview_storage(tauri::WebviewWindowBuilder::from_config(
                    app.handle(),
                    config,
                )?)
                .build()?;
            }

            // Get app data directory for persistent storage. Without it
            // nothing can be stored, so this is the one fatal setup error.
            let app_data_dir = crate::profile::app_data_dir(app.handle())
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
            // The optional answer checking model: made available in the background
            // when its files are installed and verify (it loads on first use).
            app.manage(answer_check_commands::AnswerCheckState::new(
                model_dir.clone(),
            ));
            let answer_check_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let loaded = tokio::task::spawn_blocking(move || {
                    answer_check_handle
                        .state::<answer_check_commands::AnswerCheckState>()
                        .load_if_installed()
                })
                .await;
                match loaded {
                    Ok(Ok(true)) => tracing::info!("Answer checking model available"),
                    Ok(Ok(false)) => tracing::info!(
                        "Answer checking model not installed; answers are checked for topic and numbers only"
                    ),
                    Ok(Err(e)) => tracing::warn!("{e}"),
                    Err(e) => tracing::warn!("Answer checking model load task failed: {e}"),
                }
            });

            // The optional table structure model, loaded the same way.
            app.manage(table_model_commands::TableModelState::new(model_dir.clone()));
            let table_model_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let loaded = tokio::task::spawn_blocking(move || {
                    table_model_handle
                        .state::<table_model_commands::TableModelState>()
                        .load_if_installed()
                })
                .await;
                match loaded {
                    Ok(Ok(true)) => tracing::info!("Table model available"),
                    Ok(Ok(false)) => tracing::info!(
                        "Table model not installed; tables are read with the layout heuristics only"
                    ),
                    Ok(Err(e)) => tracing::warn!("Table model not loaded: {e}"),
                    Err(e) => tracing::warn!("Table model load task failed: {e}"),
                }
            });

            // Initialize SpaceManager with persistent storage
            let space_manager = SpaceManager::with_data_dir(app_data_dir.clone());
            // Files outside a separate profile's folder belong to the normal one.
            let space_manager = if profile::active().is_separate() {
                space_manager.without_legacy_cleanup()
            } else {
                space_manager
            };

            // Create app paths
            let app_paths = AppPaths {
                data_dir: app_data_dir.clone(),
                db_path: app_data_dir.join("kalki_data"),
            };

            // Initialize LLMState FIRST so RagState can reference its manager
            let shared_llm_manager = Arc::new(AsyncRwLock::new(None));

            app.manage(model_picker_commands::ModelPickerState::default());
            app.manage(connect_commands::ConnectState::default());
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
                // Once: keys from the environment are saved in the OS
                // credential store (the environment still wins at run time).
                let migrate_handle = llm_bootstrap_handle.clone();
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    connect_commands::migrate_env_keys_once(&migrate_handle)
                })
                .await
                {
                    tracing::warn!("Environment key migration failed: {}", e);
                }
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
                let environment = match llm_bootstrap::configure_from_environment(&llm_state).await {
                    Ok(Some(configured)) => {
                        tracing::info!(
                            "LLM configured from environment: {}",
                            configured.description
                        );
                        configured.model
                    }
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!("LLM environment configuration failed: {}", e);
                        None
                    }
                };
                // The saved model choice applies unless the environment set one.
                model_picker_commands::apply_startup_choice(&llm_bootstrap_handle, environment)
                    .await;
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
            let mut default_rag = tauri::async_runtime::block_on(
                shodh_rag::comprehensive_system::ComprehensiveRAG::new(rag_config),
            )
            .map_err(|e| setup_error("Failed to open the document index", format!("{e:#}")))?;
            if !default_rag.has_search_models() {
                tracing::warn!("Search models are not installed; search needs first-run setup");
            }

            // The citation graph ranks library files for queries about papers and
            // citations; search reads the same snapshot the graph commands build.
            let graph_slot: shodh_rag::research::citations::GraphSlot = Default::default();
            default_rag.set_source_ranker(Some(Arc::new(
                shodh_rag::research::citations::GraphRanker::new(graph_slot.clone()),
            )));
            // PDFs indexed with table-candidate pages are refined in the background.
            let refinement = table_model_commands::spawn_refinement(app.handle(), &mut default_rag);
            // The embedding, reranker, answer checking and table models load on
            // first use; unload them when idle.
            if let Some(idle) = shodh_rag::lazy_model::idle_period() {
                let models: Vec<Arc<dyn shodh_rag::lazy_model::IdleUnload>> = vec![
                    default_rag.embedder_handle(),
                    default_rag.reranker_handle(),
                    app.state::<answer_check_commands::AnswerCheckState>()
                        .model
                        .clone(),
                    app.state::<table_model_commands::TableModelState>()
                        .model
                        .clone(),
                ];
                tauri::async_runtime::spawn(shodh_rag::lazy_model::unload_idle_models(
                    models, idle,
                ));
            }
            let rag_engine = Arc::new(AsyncRwLock::new(default_rag));
            table_model_commands::start_worker(app.handle().clone(), rag_engine.clone(), refinement);
            // Long-term memory: typed statements next to the document index, dynamics in
            // shodh.db. Opens on first use (it needs the search models' embedder).
            let memory_state = memory_commands::MemoryState::new(
                &app_data_dir,
                rag_engine.clone(),
                &app.state::<audit_commands::AuditState>(),
            );
            // Snippets and Result statements share the memory's statement store; images,
            // extraction reports and rejections live in shodh.db. Opens on first use.
            app.manage(research_commands::ResearchState::new(
                memory_state.clone(),
                &app.state::<audit_commands::AuditState>(),
                app.state::<table_model_commands::TableModelState>()
                    .model
                    .clone(),
                graph_slot,
            ));
            // Load a graph built in an earlier session, so search can use it from the start.
            graph_commands::warm(app.handle().clone());
            app.manage(memory_state);
            // Generated visuals (the gallery), in shodh.db. Opens on first use.
            let visual_state =
                visual_commands::VisualState::new(&app.state::<audit_commands::AuditState>());
            app.manage(visual_state);
            // Workspaces (sources, instructions), in shodh.db. Opens on first use.
            app.manage(workspace_commands::WorkspaceState::new(
                &app.state::<audit_commands::AuditState>(),
            ));
            // The Inbox (approvals, finished background work), in shodh.db.
            app.manage(inbox_commands::InboxState::new(
                &app.state::<audit_commands::AuditState>(),
            ));
            inbox_commands::recover_on_launch(app.handle());
            // Learning from conversations (needs the memory, LLM and audit states).
            memory_learn::manage(app, &app_data_dir);

            app.manage(RagState {
                rag: rag_engine,
                notes: Mutex::new(Vec::new()),
                space_manager: Mutex::new(space_manager),
                app_paths,
                rag_initialized: Arc::new(AsyncRwLock::new(false)),
                initialization_lock: Arc::new(tokio::sync::Mutex::new(())),
            });

            app.manage(IndexingState::default());
            app.manage(file_watcher::FolderSyncState::new(
                app_data_dir.join("folder_sync"),
            ));
            // Task reminders: native notifications while the app runs.
            app.manage(reminders::ReminderState::default());
            reminders::spawn(app.handle().clone());

            // Tray icon: the window can hide there and reminders keep ringing.
            app.manage(background::BackgroundState::default());
            let tray = background::create_tray(app.handle());
            if let Err(e) = &tray {
                // Without a tray, hiding would strand the window: closing quits.
                tracing::error!("Tray icon unavailable: {e}");
            }
            if tray.is_ok() && background::started_in_background() {
                if let Some(window) = app.get_webview_window(background::MAIN_WINDOW) {
                    if let Err(e) = window.hide() {
                        tracing::warn!("Could not start hidden: {e}");
                    }
                }
            }
            app.manage(agent_session_commands::AgentSessions::default());
            agent_session_commands::start_idle_reaper(app.handle().clone());
            pdf_export::manage(app.handle());
            let analytics_path = app_data_dir.join("analytics.json");
            app.manage(AnalyticsState::load_or_default(&analytics_path));
            app.manage(TemplateStore::default());

            // MCP servers and skills for the agent (Settings → Tools & connections).
            app.manage(mcp_commands::McpState(mcp::McpManager::new(
                app_data_dir.clone(),
            )));
            app.manage(mcp::skills::SkillInstaller::default());

            // Initialize search and chat history managers
            let search_history_manager = SearchHistoryManager::new(&app_data_dir);
            app.manage(Arc::new(Mutex::new(search_history_manager)));

            let chat_history_manager = ChatHistoryManager::new(&app_data_dir);
            app.manage(Arc::new(Mutex::new(chat_history_manager)));

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
            answer_check_commands::answer_check_status,
            answer_check_commands::install_answer_check_model,
            table_model_commands::table_model_status,
            table_model_commands::install_table_model,
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
            pdf_export::export_pdf,
            pdf_export::print_job,
            pdf_export::print_job_ready,
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
            file_watcher::sync_folder_sources,
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
            model_picker_commands::model_picker_view,
            model_picker_commands::model_select,
            model_picker_commands::model_set_favourite,
            model_picker_commands::model_set_fallback,
            model_picker_commands::model_fallback_offer,
            model_picker_commands::model_use_pick,
            model_picker_commands::model_set_provider_order,
            model_picker_commands::model_set_base_url,
            connect_commands::connect_sign_in,
            connect_commands::connect_sign_in_cancel,
            connect_commands::connect_sign_out,
            connect_commands::connect_save_key,
            connect_commands::connect_remove_key,
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
            // Document commands (in rag_commands.rs)
            rag_commands::get_document_preview,
            source_viewer_commands::get_source_file_info,
            source_viewer_commands::read_source_bytes,
            source_viewer_commands::read_source_text,
            source_viewer_commands::read_source_table,
            source_viewer_commands::get_pdf_info,
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
            // Tools & connections: MCP servers, skills, the composer tool chip
            mcp_commands::tools_overview,
            mcp_commands::mcp_test_server,
            mcp_commands::mcp_add_servers,
            mcp_commands::mcp_remove_server,
            mcp_commands::mcp_set_server,
            mcp_commands::mcp_set_tool,
            mcp_commands::mcp_open_config,
            mcp_commands::enola_install,
            mcp_commands::skills_prepare_install,
            mcp_commands::skills_prepare_recommended,
            mcp_commands::skills_set_modes,
            mcp_commands::skills_confirm_install,
            mcp_commands::skills_cancel_install,
            mcp_commands::skills_set_enabled,
            mcp_commands::skills_remove,
            mcp_commands::tools_for_chat,
            // Document Upload commands
            document_upload_commands::upload_document_file,
            document_upload_commands::save_temp_file,
            // Agent sessions (omp harness)
            agent_session_commands::agent_start,
            agent_session_commands::agent_send,
            agent_session_commands::agent_steer,
            agent_session_commands::agent_abort,
            agent_session_commands::agent_approve,
            agent_session_commands::agent_code_status,
            agent_session_commands::agent_code_discard,
            agent_session_commands::agent_code_paths,
            agent_session_commands::code_settings_get,
            agent_session_commands::code_settings_set,
            agent_session_commands::agent_install_runtime,
            audit_commands::audit_query,
            audit_commands::audit_verify,
            audit_commands::audit_export,
            audit_commands::audit_set_retention_days,
            audit_commands::audit_stats,
            agent_session_commands::agent_runtime_status,
            agent_session_commands::agent_close_session,
            agent_session_commands::agent_session_counts,
            // Conversation persistence commands
            conversation_commands::load_conversations,
            conversation_commands::save_conversation,
            conversation_commands::delete_conversation,
            conversation_commands::rename_conversation,
            conversation_commands::pin_conversation,
            // Library file browser
            library_commands::list_directory,
            // App settings (preferences: user and agent; policy: user only)
            app_settings::get_app_settings,
            app_settings::update_app_preferences,
            app_settings::set_app_policy,
            app_settings::set_memory_preferences,
            app_settings::set_answer_preferences,
            // Long-term memory (Settings → Memory)
            memory_commands::memory_list,
            memory_commands::memory_history,
            memory_commands::memory_update,
            memory_commands::memory_set_pinned,
            memory_commands::memory_forget,
            memory_commands::memory_export,
            memory_learn::memory_learn_status,
            memory_learn::memory_suggestions_list,
            memory_learn::memory_suggestion_accept,
            memory_learn::memory_suggestions_accept_many,
            memory_learn::memory_suggestion_reject,
            memory_learn::memory_suggestion_undo,
            memory_learn::memory_learning_stop,
            memory_learn::memory_consolidate_now,
            // Generated visuals (the gallery and the focus pop-out)
            visual_commands::visuals_capture,
            inbox_commands::inbox_list,
            inbox_commands::inbox_dismiss,
            workspace_commands::workspaces_list,
            workspace_commands::workspaces_templates,
            workspace_commands::workspaces_get,
            workspace_commands::workspaces_create,
            workspace_commands::workspaces_update,
            workspace_commands::workspaces_set_archived,
            workspace_commands::workspaces_set_instructions,
            workspace_commands::workspaces_instruction_history,
            workspace_commands::workspaces_add_sources,
            workspace_commands::workspaces_remove_source,
            workspace_commands::workspaces_delete,
            workspace_commands::workspaces_source_health,
            visual_commands::visuals_backfill_status,
            visual_commands::visuals_backfill,
            visual_commands::visuals_list,
            visual_commands::visuals_count,
            visual_commands::visuals_get,
            visual_commands::visuals_rename,
            visual_commands::visuals_set_pinned,
            visual_commands::visuals_set_note,
            visual_commands::visuals_add_version,
            visual_commands::visuals_delete,
            visual_commands::visuals_restore,
            research_commands::snippets_create,
            research_commands::snippets_list,
            research_commands::snippets_get,
            research_commands::snippets_image,
            research_commands::snippets_set_image,
            research_commands::snippets_update,
            research_commands::snippets_delete,
            research_commands::snippets_table,
            research_commands::snippets_transcribe_latex,
            research_commands::vision_capability,
            research_commands::results_extract,
            research_commands::results_list,
            research_commands::results_review,
            research_commands::results_query,
            research_commands::results_facets,
            research_commands::paper_objects,
            graph_commands::paper_graph_status,
            graph_commands::paper_graph_build,
            graph_commands::paper_graph_view,
            graph_commands::paper_get,
            graph_commands::papers_find,
            graph_commands::paper_concept,
            // Calendar/Todo commands
            calendar_commands::load_tasks,
            calendar_commands::create_task,
            calendar_commands::update_task,
            calendar_commands::delete_task,
            calendar_commands::add_subtask,
            calendar_commands::toggle_subtask,
            calendar_commands::rename_subtask,
            calendar_commands::delete_subtask,
            calendar_commands::load_events,
            calendar_commands::create_event,
            calendar_commands::update_event,
            calendar_commands::delete_event,
            // Task reminders
            reminders::snooze_reminder,
            reminders::list_missed_reminders,
            reminders::dismiss_missed_reminders,
            // Background mode (tray, start with Windows)
            background::get_background_status,
            background::set_close_to_tray,
            background::set_start_with_windows,
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

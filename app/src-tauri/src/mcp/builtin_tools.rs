//! Built-in MCP tools that expose Shodh-RAG functionality

use super::*;
use serde_json::json;

/// Get all built-in Shodh-RAG tools
pub fn get_builtin_tools() -> Vec<ToolDefinition> {
    vec![
        // RAG Search tool
        ToolDefinition {
            name: "rag_search".to_string(),
            description: "Search through indexed documents using semantic search, hybrid search, or keyword search".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return",
                        "default": 5
                    },
                    "space_id": {
                        "type": "string",
                        "description": "Optional space ID to filter results"
                    },
                    "filters": {
                        "type": "object",
                        "description": "Optional metadata filters",
                        "properties": {
                            "source_type": {"type": "string"},
                            "author": {"type": "string"},
                            "date_start": {"type": "string"},
                            "date_end": {"type": "string"}
                        }
                    }
                },
                "required": ["query"]
            }),
            category: Some(ToolCategory::RagSystem),
        },

        // Knowledge Graph Query
        ToolDefinition {
            name: "knowledge_graph_query".to_string(),
            description: "Query the knowledge graph for entities, relationships, and context".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The knowledge graph query"
                    },
                    "max_items": {
                        "type": "integer",
                        "description": "Maximum number of items to return",
                        "default": 10
                    }
                },
                "required": ["query"]
            }),
            category: Some(ToolCategory::RagSystem),
        },

        // Memory Retrieval
        ToolDefinition {
            name: "retrieve_memory".to_string(),
            description: "Retrieve relevant memories from the memory system based on query or time range".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Optional query to search memories"
                    },
                    "time_range_hours": {
                        "type": "integer",
                        "description": "Retrieve memories from last N hours",
                        "default": 24
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of memories to return",
                        "default": 5
                    }
                }
            }),
            category: Some(ToolCategory::RagSystem),
        },

        // Document Upload
        ToolDefinition {
            name: "index_document".to_string(),
            description: "Index a new document into the RAG system".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the document file"
                    },
                    "space_id": {
                        "type": "string",
                        "description": "Space ID to add document to"
                    },
                    "metadata": {
                        "type": "object",
                        "description": "Optional metadata for the document"
                    }
                },
                "required": ["file_path", "space_id"]
            }),
            category: Some(ToolCategory::RagSystem),
        },

        // Web Search
        ToolDefinition {
            name: "web_search".to_string(),
            description: "Search the web using DuckDuckGo and return results".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The web search query"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
            category: Some(ToolCategory::Search),
        },

        // Hybrid Search (Local + Web)
        ToolDefinition {
            name: "hybrid_search".to_string(),
            description: "Perform hybrid search combining local RAG and web search".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query"
                    },
                    "local_results": {
                        "type": "integer",
                        "description": "Number of local results",
                        "default": 3
                    },
                    "web_results": {
                        "type": "integer",
                        "description": "Number of web results",
                        "default": 2
                    }
                },
                "required": ["query"]
            }),
            category: Some(ToolCategory::Search),
        },

        // Get Space Statistics
        ToolDefinition {
            name: "get_statistics".to_string(),
            description: "Get RAG system statistics including document count, index size, etc.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
            category: Some(ToolCategory::RagSystem),
        },

        // Chat with Codebase
        ToolDefinition {
            name: "chat_with_codebase".to_string(),
            description: "Ask questions about code in the indexed codebase with full context".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "The question about the codebase"
                    },
                    "space_id": {
                        "type": "string",
                        "description": "Optional space ID to filter by"
                    }
                },
                "required": ["question"]
            }),
            category: Some(ToolCategory::Development),
        },

        // Visualize Call Graph
        ToolDefinition {
            name: "visualize_call_graph".to_string(),
            description: "Generate an interactive call graph visualization showing how functions interact in the codebase. Use this when users ask to visualize, trace, or understand execution flow, architecture, or function relationships.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query to find entry point function or feature (e.g., 'authentication', 'login flow', 'file upload', 'search')"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum depth to traverse in the call hierarchy (1-10, default: 5)",
                        "default": 5,
                        "minimum": 1,
                        "maximum": 10
                    },
                    "format": {
                        "type": "string",
                        "enum": ["interactive", "ascii"],
                        "description": "Output format: 'interactive' for web visualization with D3.js or 'ascii' for text-based tree",
                        "default": "interactive"
                    },
                    "space_id": {
                        "type": "string",
                        "description": "Optional space ID to limit search to specific codebase"
                    }
                },
                "required": ["query"]
            }),
            category: Some(ToolCategory::Development),
        },

        // Read Original File
        ToolDefinition {
            name: "read_file".to_string(),
            description: "Read the full content of an indexed file".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    }
                },
                "required": ["file_path"]
            }),
            category: Some(ToolCategory::Filesystem),
        },

        // List Documents
        ToolDefinition {
            name: "list_documents".to_string(),
            description: "List all indexed documents in a space".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "space_id": {
                        "type": "string",
                        "description": "Space ID to list documents from"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of documents to return",
                        "default": 100
                    }
                },
                "required": ["space_id"]
            }),
            category: Some(ToolCategory::RagSystem),
        },
    ]
}

/// Default MCP server configurations for popular integrations
pub fn get_default_mcp_servers() -> Vec<MCPServerConfig> {
    vec![
        // Filesystem MCP Server
        MCPServerConfig {
            name: "filesystem".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-filesystem".to_string(),
                "C:\\".to_string(), // Root directory - can be configured
            ],
            env: HashMap::new(),
            transport: TransportType::Stdio,
        },

        // GitHub MCP Server
        MCPServerConfig {
            name: "github".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string(),
            ],
            env: {
                let mut env = HashMap::new();
                // User needs to provide their GitHub token
                env.insert("GITHUB_PERSONAL_ACCESS_TOKEN".to_string(), "".to_string());
                env
            },
            transport: TransportType::Stdio,
        },

        // Brave Search MCP Server
        MCPServerConfig {
            name: "brave-search".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-brave-search".to_string(),
            ],
            env: {
                let mut env = HashMap::new();
                env.insert("BRAVE_API_KEY".to_string(), "".to_string());
                env
            },
            transport: TransportType::Stdio,
        },

        // Postgres MCP Server
        MCPServerConfig {
            name: "postgres".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-postgres".to_string(),
            ],
            env: {
                let mut env = HashMap::new();
                env.insert("POSTGRES_CONNECTION_STRING".to_string(), "".to_string());
                env
            },
            transport: TransportType::Stdio,
        },

        // Google Drive MCP Server
        MCPServerConfig {
            name: "google-drive".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-google-drive".to_string(),
            ],
            env: HashMap::new(),
            transport: TransportType::Stdio,
        },

        // Slack MCP Server
        MCPServerConfig {
            name: "slack".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-slack".to_string(),
            ],
            env: {
                let mut env = HashMap::new();
                env.insert("SLACK_BOT_TOKEN".to_string(), "".to_string());
                env
            },
            transport: TransportType::Stdio,
        },
    ]
}

//! Metadata management and access control for enterprise RAG
//! Handles department-level isolation, access levels, and compliance

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Document metadata for enterprise filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    /// Unique document identifier
    pub doc_id: Uuid,

    /// Chunk identifier within document
    pub chunk_id: usize,

    /// Unix timestamp
    pub timestamp: i64,

    /// Department (HR, Finance, Engineering, etc.)
    pub department: Option<String>,

    /// Access level (0=public, 1=internal, 2=confidential, 3=restricted)
    pub access_level: AccessLevel,

    /// Language code (ISO 639-1)
    pub language: String,

    /// Source type
    pub source_type: SourceType,

    /// Custom tags
    pub tags: Vec<String>,

    /// Author/owner
    pub author: Option<String>,

    /// Compliance flags
    pub compliance: ComplianceFlags,

    /// Custom fields for additional metadata
    pub custom_fields: std::collections::HashMap<String, String>,
}

impl Default for DocumentMetadata {
    fn default() -> Self {
        Self {
            doc_id: Uuid::new_v4(),
            chunk_id: 0,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            department: None,
            access_level: AccessLevel::Public,
            language: "en".to_string(),
            source_type: SourceType::TXT,
            tags: Vec::new(),
            author: None,
            compliance: ComplianceFlags::default(),
            custom_fields: std::collections::HashMap::new(),
        }
    }
}

impl DocumentMetadata {
    /// Create new metadata with ID
    pub fn new(doc_id: Uuid) -> Self {
        Self {
            doc_id,
            ..Default::default()
        }
    }

    /// Builder pattern for fluent API
    pub fn with_department(mut self, dept: impl Into<String>) -> Self {
        self.department = Some(dept.into());
        self
    }

    pub fn with_access_level(mut self, level: AccessLevel) -> Self {
        self.access_level = level;
        self
    }

    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    pub fn with_source(mut self, source: SourceType) -> Self {
        self.source_type = source;
        self
    }
}

/// Access levels for documents
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(u8)]
pub enum AccessLevel {
    Public = 0,
    Internal = 1,
    Confidential = 2,
    Restricted = 3,
}

impl AccessLevel {
    pub fn from_u8(level: u8) -> Self {
        match level {
            0 => Self::Public,
            1 => Self::Internal,
            2 => Self::Confidential,
            3 => Self::Restricted,
            _ => Self::Public,
        }
    }
}

/// Document source types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SourceType {
    PDF,
    DOCX,
    TXT,
    XML,
    HTML,
    Email,
    Confluence,
    SharePoint,
    Teams,
    Slack,
    Database,
    API,
    File, // Generic file source
    WEB,  // Web search result
}

/// Compliance flags for regulatory requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceFlags {
    pub pii_present: bool,           // Contains Personally Identifiable Information
    pub gdpr_relevant: bool,         // Subject to GDPR
    pub hipaa_relevant: bool,        // Subject to HIPAA
    pub financial_data: bool,        // Contains financial information
    pub retention_days: Option<u32>, // Data retention period
}

impl Default for ComplianceFlags {
    fn default() -> Self {
        Self {
            pii_present: false,
            gdpr_relevant: false,
            hipaa_relevant: false,
            financial_data: false,
            retention_days: None,
        }
    }
}

/// Filter for metadata-based search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataFilter {
    /// Filter by departments
    pub departments: Option<Vec<String>>,

    /// Maximum access level user can see
    pub max_access_level: AccessLevel,

    /// Filter by languages
    pub languages: Option<Vec<String>>,

    /// Date range (start, end) as Unix timestamps
    pub date_range: Option<(i64, i64)>,

    /// Required tags (AND condition)
    pub required_tags: Option<Vec<String>>,

    /// Any of these tags (OR condition)
    pub any_tags: Option<Vec<String>>,

    /// Excluded tags
    pub excluded_tags: Option<Vec<String>>,

    /// Filter by source types
    pub source_types: Option<Vec<SourceType>>,

    /// Filter by author
    pub authors: Option<Vec<String>>,

    /// Compliance requirements
    pub compliance_filter: Option<ComplianceFilter>,
}

impl MetadataFilter {
    /// Check if metadata matches filter
    pub fn matches(&self, metadata: &DocumentMetadata) -> bool {
        // Check department
        if let Some(ref depts) = self.departments {
            if let Some(ref dept) = metadata.department {
                if !depts.contains(dept) {
                    return false;
                }
            } else {
                return false; // No department but filter requires one
            }
        }

        // Check access level
        if metadata.access_level > self.max_access_level {
            return false;
        }

        // Check language
        if let Some(ref langs) = self.languages {
            if !langs.contains(&metadata.language) {
                return false;
            }
        }

        // Check date range
        if let Some((start, end)) = self.date_range {
            if metadata.timestamp < start || metadata.timestamp > end {
                return false;
            }
        }

        // Check required tags (AND)
        if let Some(ref req_tags) = self.required_tags {
            for tag in req_tags {
                if !metadata.tags.contains(tag) {
                    return false;
                }
            }
        }

        // Check any tags (OR)
        if let Some(ref any_tags) = self.any_tags {
            let mut found = false;
            for tag in any_tags {
                if metadata.tags.contains(tag) {
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }

        // Check excluded tags
        if let Some(ref excl_tags) = self.excluded_tags {
            for tag in excl_tags {
                if metadata.tags.contains(tag) {
                    return false;
                }
            }
        }

        // Check source type
        if let Some(ref types) = self.source_types {
            if !types.is_empty() && !types.contains(&metadata.source_type) {
                return false;
            }
        }

        // Check author
        if let Some(ref authors) = self.authors {
            if let Some(ref author) = metadata.author {
                if !authors.contains(author) {
                    return false;
                }
            } else {
                return false;
            }
        }

        // Check compliance
        if let Some(ref comp_filter) = self.compliance_filter {
            if !comp_filter.matches(&metadata.compliance) {
                return false;
            }
        }

        true
    }
}

/// Compliance-based filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceFilter {
    pub exclude_pii: bool,
    pub gdpr_only: bool,
    pub hipaa_only: bool,
    pub exclude_financial: bool,
    pub min_retention_days: Option<u32>,
}

impl ComplianceFilter {
    pub fn matches(&self, flags: &ComplianceFlags) -> bool {
        if self.exclude_pii && flags.pii_present {
            return false;
        }

        if self.gdpr_only && !flags.gdpr_relevant {
            return false;
        }

        if self.hipaa_only && !flags.hipaa_relevant {
            return false;
        }

        if self.exclude_financial && flags.financial_data {
            return false;
        }

        if let Some(min_days) = self.min_retention_days {
            if let Some(retention) = flags.retention_days {
                if retention < min_days {
                    return false;
                }
            } else {
                return false; // No retention period set
            }
        }

        true
    }
}

/// User context for access control
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserContext {
    pub user_id: String,
    pub email: String,
    pub departments: Vec<String>,
    pub access_level: AccessLevel,
    pub roles: Vec<String>,
    pub languages: Vec<String>,
}

impl UserContext {
    /// Create metadata filter based on user context
    pub fn to_filter(&self) -> MetadataFilter {
        MetadataFilter {
            departments: Some(self.departments.clone()),
            max_access_level: self.access_level,
            languages: Some(self.languages.clone()),
            date_range: None,
            required_tags: None,
            any_tags: None,
            excluded_tags: None,
            source_types: None,
            authors: None,
            compliance_filter: None,
        }
    }

    /// Check if user can access document
    pub fn can_access(&self, metadata: &DocumentMetadata) -> bool {
        // Check department access
        if let Some(ref dept) = metadata.department {
            if !self.departments.contains(dept) {
                // Check for cross-department roles
                if !self.roles.contains(&"admin".to_string()) {
                    return false;
                }
            }
        }

        // Check access level
        if metadata.access_level > self.access_level {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_control() {
        let user = UserContext {
            user_id: "user123".to_string(),
            email: "user@company.com".to_string(),
            departments: vec!["Engineering".to_string()],
            access_level: AccessLevel::Internal,
            roles: vec!["developer".to_string()],
            languages: vec!["en".to_string(), "hi".to_string()],
        };

        let mut metadata = DocumentMetadata {
            doc_id: Uuid::new_v4(),
            chunk_id: 0,
            timestamp: 1234567890,
            department: Some("Engineering".to_string()),
            access_level: AccessLevel::Internal,
            language: "en".to_string(),
            source_type: SourceType::PDF,
            tags: vec!["technical".to_string()],
            author: Some("john@company.com".to_string()),
            compliance: ComplianceFlags::default(),
            custom_fields: std::collections::HashMap::new(),
        };

        assert!(user.can_access(&metadata));

        // User shouldn't access restricted documents
        metadata.access_level = AccessLevel::Restricted;
        assert!(!user.can_access(&metadata));

        // User shouldn't access other departments
        metadata.access_level = AccessLevel::Internal;
        metadata.department = Some("Finance".to_string());
        assert!(!user.can_access(&metadata));
    }

    #[test]
    fn test_metadata_filter() {
        let filter = MetadataFilter {
            departments: Some(vec!["HR".to_string()]),
            max_access_level: AccessLevel::Confidential,
            languages: Some(vec!["en".to_string()]),
            date_range: Some((1000000000, 2000000000)),
            required_tags: Some(vec!["policy".to_string()]),
            any_tags: None,
            excluded_tags: Some(vec!["draft".to_string()]),
            source_types: None,
            authors: None,
            compliance_filter: None,
        };

        let metadata = DocumentMetadata {
            doc_id: Uuid::new_v4(),
            chunk_id: 0,
            timestamp: 1500000000,
            department: Some("HR".to_string()),
            access_level: AccessLevel::Internal,
            language: "en".to_string(),
            source_type: SourceType::PDF,
            tags: vec!["policy".to_string(), "approved".to_string()],
            author: None,
            compliance: ComplianceFlags::default(),
            custom_fields: std::collections::HashMap::new(),
        };

        assert!(filter.matches(&metadata));
    }
}

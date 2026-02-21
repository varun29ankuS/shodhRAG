import React, { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { motion, AnimatePresence } from 'framer-motion';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useTheme } from '../contexts/ThemeContext';
import { FileText, Plus, Trash2, Eye, Layers, Check, Copy, ArrowLeft, Loader2 } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle } from './ui/card';

interface TemplateSection {
  name: string;
  order: number;
  content_type: string;
  placeholder: string;
  is_required: boolean;
  formatting_rules: string[];
}

interface TemplateMetadata {
  document_type: string;
  industry?: string;
  language: string;
  created_from: string[];
  confidence_score: number;
  usage_count: number;
}

interface TemplateVariable {
  name: string;
  description: string;
  default_value?: string;
  validation_pattern?: string;
}

interface DocumentTemplate {
  id: string;
  name: string;
  description: string;
  sections: TemplateSection[];
  metadata: TemplateMetadata;
  variables: TemplateVariable[];
  example_content: string;
}

interface DocumentInfo {
  id: string;
  title: string;
  file_path?: string;
  chunk_count: number;
  space_id?: string;
}

type WorkflowStep = 'select' | 'extract' | 'library' | 'generate' | 'preview';

interface SmartTemplatesProps {
  spaceId?: string;
}

export const SmartTemplates: React.FC<SmartTemplatesProps> = ({ spaceId }) => {
  const { colors } = useTheme();
  const [step, setStep] = useState<WorkflowStep>('library');
  const [selectedDocuments, setSelectedDocuments] = useState<string[]>([]);
  const [availableDocuments, setAvailableDocuments] = useState<DocumentInfo[]>([]);
  const [templates, setTemplates] = useState<DocumentTemplate[]>([]);
  const [selectedTemplate, setSelectedTemplate] = useState<DocumentTemplate | null>(null);
  const [templateName, setTemplateName] = useState('');
  const [autoDetectSections, setAutoDetectSections] = useState(true);
  const [variables, setVariables] = useState<Record<string, string>>({});
  const [outputFormat, setOutputFormat] = useState('markdown');
  const [generatedContent, setGeneratedContent] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    loadTemplates();
  }, []);

  const loadTemplates = async () => {
    try {
      const result = await invoke<DocumentTemplate[]>('list_templates');
      setTemplates(result);
    } catch (err) {
      console.error('Failed to load templates:', err);
    }
  };

  const loadDocuments = async () => {
    try {
      setLoading(true);
      const docs = await invoke<DocumentInfo[]>('get_comparable_documents', {
        spaceId: spaceId || null  // Use active workspace context
      });
      setAvailableDocuments(docs);
    } catch (err) {
      setError('Failed to load documents: ' + String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleExtractTemplate = async () => {
    if (selectedDocuments.length === 0) {
      setError('Please select at least one document');
      return;
    }

    if (!templateName.trim()) {
      setError('Please provide a template name');
      return;
    }

    try {
      setLoading(true);
      setError('');

      const template = await invoke<DocumentTemplate>('extract_template', {
        documentIds: selectedDocuments,
        templateName,
        autoDetectSections,
      });

      setTemplates([...templates, template]);
      setSelectedTemplate(template);
      setStep('library');
      setTemplateName('');
      setSelectedDocuments([]);
    } catch (err) {
      setError('Failed to extract template: ' + String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleGenerateFromTemplate = async () => {
    if (!selectedTemplate) {
      setError('No template selected');
      return;
    }

    try {
      setLoading(true);
      setError('');

      const content = await invoke<string>('generate_from_template', {
        templateId: selectedTemplate.id,
        variables,
        outputFormat,
      });

      setGeneratedContent(content);
      setStep('preview');
    } catch (err) {
      setError('Failed to generate document: ' + String(err));
    } finally {
      setLoading(false);
    }
  };

  const handleDeleteTemplate = async (templateId: string) => {
    try {
      await invoke('delete_template', { templateId });
      setTemplates(templates.filter(t => t.id !== templateId));
      if (selectedTemplate?.id === templateId) {
        setSelectedTemplate(null);
      }
    } catch (err) {
      setError('Failed to delete template: ' + String(err));
    }
  };

  const renderSelectDocuments = () => (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      className="space-y-6"
    >
      <div>
        <h2 className="text-2xl font-bold mb-2" style={{ color: colors.text }}>
          Select Documents for Template
        </h2>
        <p className="text-sm" style={{ color: colors.textSecondary }}>
          Choose one or more documents to extract a template from
        </p>
      </div>

      <div className="space-y-4">
        <div>
          <label className="text-sm font-medium mb-2 block" style={{ color: colors.textMuted }}>
            TEMPLATE NAME
          </label>
          <input
            type="text"
            placeholder="e.g., Technical Report Template"
            value={templateName}
            onChange={(e) => setTemplateName(e.target.value)}
            className="w-full px-4 py-2.5 rounded-lg outline-none transition-colors duration-200"
            style={{
              backgroundColor: colors.inputBg,
              borderWidth: '1px',
              borderStyle: 'solid',
              borderColor: colors.border,
              color: colors.text
            }}
            onFocus={(e) => e.target.style.borderColor = colors.primary}
            onBlur={(e) => e.target.style.borderColor = colors.border}
          />
        </div>

        <label className="flex items-center gap-2 cursor-pointer">
          <input
            type="checkbox"
            checked={autoDetectSections}
            onChange={(e) => setAutoDetectSections(e.target.checked)}
            className="w-4 h-4 rounded"
            style={{
              accentColor: colors.primary
            }}
          />
          <span className="text-sm" style={{ color: colors.text }}>
            Auto-detect sections
          </span>
        </label>
      </div>

      {availableDocuments.length === 0 ? (
        <div className="text-center py-8" style={{ backgroundColor: colors.bgSecondary }}>
          <FileText className="w-12 h-12 mx-auto mb-4" style={{ color: colors.textMuted }} />
          <button
            onClick={loadDocuments}
            className="px-4 py-2 rounded-lg font-medium transition-colors duration-200"
            style={{
              backgroundColor: colors.primary,
              color: colors.primaryText
            }}
          >
            Load Documents
          </button>
        </div>
      ) : (
        <div className="space-y-2">
          <h3 className="text-sm font-semibold" style={{ color: colors.textMuted }}>
            SELECT DOCUMENTS ({selectedDocuments.length} selected)
          </h3>
          <div className="space-y-2 max-h-96 overflow-y-auto">
            {availableDocuments.map((doc) => (
              <div
                key={doc.id}
                onClick={() => {
                  if (selectedDocuments.includes(doc.id)) {
                    setSelectedDocuments(selectedDocuments.filter(id => id !== doc.id));
                  } else {
                    setSelectedDocuments([...selectedDocuments, doc.id]);
                  }
                }}
                className="p-4 rounded-lg border-2 cursor-pointer transition-all duration-200"
                style={{
                  backgroundColor: selectedDocuments.includes(doc.id) ? colors.bgTertiary : 'transparent',
                  borderColor: selectedDocuments.includes(doc.id) ? colors.primary : colors.border
                }}
              >
                <div className="flex items-start gap-3">
                  <div
                    className="w-5 h-5 rounded border-2 flex items-center justify-center flex-shrink-0 mt-0.5"
                    style={{
                      borderColor: selectedDocuments.includes(doc.id) ? colors.primary : colors.border,
                      backgroundColor: selectedDocuments.includes(doc.id) ? colors.primary : 'transparent'
                    }}
                  >
                    {selectedDocuments.includes(doc.id) && <Check size={14} style={{ color: '#ffffff' }} />}
                  </div>
                  <div className="flex-1 min-w-0">
                    <div className="font-medium truncate" style={{ color: colors.text }}>{doc.title}</div>
                    <div className="text-xs mt-1" style={{ color: colors.textMuted }}>
                      {doc.chunk_count} chunks
                    </div>
                  </div>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="flex gap-3">
        <button
          onClick={() => setStep('library')}
          className="flex-1 py-2.5 rounded-lg font-medium transition-colors duration-200"
          style={{
            backgroundColor: colors.bgTertiary,
            color: colors.text,
            borderWidth: '1px',
            borderStyle: 'solid',
            borderColor: colors.border
          }}
        >
          Cancel
        </button>
        <button
          onClick={handleExtractTemplate}
          disabled={selectedDocuments.length === 0 || !templateName.trim() || loading}
          className="flex-1 py-2.5 rounded-lg font-medium disabled:opacity-50 disabled:cursor-not-allowed transition-colors duration-200"
          style={{
            backgroundColor: colors.primary,
            color: colors.primaryText
          }}
        >
          {loading ? 'Extracting...' : 'Extract Template'}
        </button>
      </div>
    </motion.div>
  );

  const renderTemplateLibrary = () => (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      className="space-y-6"
    >
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold" style={{ color: colors.text }}>Template Library</h2>
        <button
          onClick={() => {
            loadDocuments();
            setStep('select');
          }}
          className="px-4 py-2 rounded-lg font-medium flex items-center gap-2 transition-colors duration-200"
          style={{
            backgroundColor: colors.primary,
            color: colors.primaryText
          }}
        >
          <Plus className="w-4 h-4" />
          Create New Template
        </button>
      </div>

      {templates.length === 0 ? (
        <div className="text-center py-16" style={{ backgroundColor: colors.bgSecondary }}>
          <Layers className="w-16 h-16 mx-auto mb-4" style={{ color: colors.textMuted }} />
          <p className="text-lg" style={{ color: colors.textSecondary }}>
            No templates yet. Create your first template from existing documents.
          </p>
        </div>
      ) : (
        <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
          {templates.map((template) => (
            <motion.div
              key={template.id}
              layout
              initial={{ opacity: 0, scale: 0.95 }}
              animate={{ opacity: 1, scale: 1 }}
              className="p-4 rounded-lg border-2 transition-all duration-200"
              style={{
                backgroundColor: colors.cardBg,
                borderColor: colors.cardBorder
              }}
            >
              <div className="flex items-start justify-between mb-3">
                <h3 className="font-bold text-lg" style={{ color: colors.text }}>
                  {template.name}
                </h3>
                <button
                  onClick={(e) => {
                    e.stopPropagation();
                    handleDeleteTemplate(template.id);
                  }}
                  className="p-1 rounded transition-colors duration-200"
                  style={{ color: colors.textMuted }}
                  onMouseEnter={(e) => {
                    e.currentTarget.style.backgroundColor = colors.error + '20';
                    e.currentTarget.style.color = colors.error;
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.backgroundColor = 'transparent';
                    e.currentTarget.style.color = colors.textMuted;
                  }}
                >
                  <Trash2 className="w-4 h-4" />
                </button>
              </div>

              <p className="text-sm mb-4 line-clamp-2" style={{ color: colors.textSecondary }}>
                {template.description}
              </p>

              <div className="flex gap-2 mb-4 flex-wrap">
                <span
                  className="text-xs px-2 py-1 rounded"
                  style={{
                    backgroundColor: colors.primary + '20',
                    color: colors.primary
                  }}
                >
                  {template.metadata.document_type}
                </span>
                <span
                  className="text-xs px-2 py-1 rounded"
                  style={{
                    backgroundColor: colors.bgTertiary,
                    color: colors.textMuted
                  }}
                >
                  {template.sections.length} sections
                </span>
                <span
                  className="text-xs px-2 py-1 rounded"
                  style={{
                    backgroundColor: colors.bgTertiary,
                    color: colors.textMuted
                  }}
                >
                  {template.variables.length} variables
                </span>
              </div>

              <div className="mb-4">
                <h4 className="text-xs font-semibold mb-2" style={{ color: colors.textMuted }}>
                  SECTIONS:
                </h4>
                <ul className="space-y-1">
                  {template.sections.slice(0, 3).map((section) => (
                    <li key={section.order} className="text-sm flex items-center gap-2" style={{ color: colors.text }}>
                      <span className="w-1 h-1 rounded-full" style={{ backgroundColor: colors.primary }} />
                      {section.name}
                    </li>
                  ))}
                  {template.sections.length > 3 && (
                    <li className="text-sm" style={{ color: colors.textMuted }}>
                      +{template.sections.length - 3} more
                    </li>
                  )}
                </ul>
              </div>

              <button
                onClick={() => {
                  setSelectedTemplate(template);
                  setStep('generate');
                }}
                className="w-full py-2 rounded-lg font-medium transition-colors duration-200"
                style={{
                  backgroundColor: colors.bgTertiary,
                  color: colors.text,
                  borderWidth: '1px',
                  borderStyle: 'solid',
                  borderColor: colors.border
                }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.backgroundColor = colors.primary;
                  e.currentTarget.style.color = colors.primaryText;
                  e.currentTarget.style.borderColor = colors.primary;
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.backgroundColor = colors.bgTertiary;
                  e.currentTarget.style.color = colors.text;
                  e.currentTarget.style.borderColor = colors.border;
                }}
              >
                Use Template
              </button>
            </motion.div>
          ))}
        </div>
      )}
    </motion.div>
  );

  const renderGenerateDocument = () => {
    if (!selectedTemplate) return null;

    const formatOptions = [
      { value: 'markdown', label: 'Markdown' },
      { value: 'html', label: 'HTML' },
      { value: 'text', label: 'Plain Text' },
      { value: 'json', label: 'JSON' },
    ];

    return (
      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        className="space-y-4"
      >
        <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
          <CardHeader>
            <CardTitle className="flex items-center gap-2" style={{ color: colors.text }}>
              <FileText className="w-5 h-5" style={{ color: colors.primary }} />
              Generate from: {selectedTemplate.name}
            </CardTitle>
            <p className="text-sm mt-1" style={{ color: colors.textMuted }}>
              {selectedTemplate.description}
            </p>
          </CardHeader>
          <CardContent className="space-y-5">

            {/* Template Structure */}
            <div>
              <label className="text-xs font-semibold mb-2 block" style={{ color: colors.textMuted }}>
                TEMPLATE STRUCTURE ({selectedTemplate.sections.length} sections)
              </label>
              <div className="space-y-1.5">
                {selectedTemplate.sections.map((section, idx) => (
                  <div
                    key={section.order}
                    className="flex items-center gap-2 px-3 py-2 rounded-lg"
                    style={{ backgroundColor: colors.bgTertiary }}
                  >
                    <span className="text-xs font-mono" style={{ color: colors.textMuted }}>{idx + 1}.</span>
                    <span className="text-sm font-medium" style={{ color: colors.text }}>{section.name}</span>
                    <span className="text-xs px-1.5 py-0.5 rounded" style={{
                      backgroundColor: `${colors.primary}15`,
                      color: colors.primary,
                    }}>
                      {section.content_type}
                    </span>
                    {section.is_required && (
                      <span className="text-xs font-medium" style={{ color: colors.error }}>*</span>
                    )}
                  </div>
                ))}
              </div>
            </div>

            {/* Variables Form */}
            {selectedTemplate.variables.length > 0 && (
              <div>
                <label className="text-xs font-semibold mb-2 block" style={{ color: colors.textMuted }}>
                  FILL IN VARIABLES ({selectedTemplate.variables.length})
                </label>
                <div className="space-y-3">
                  {selectedTemplate.variables.map((variable) => (
                    <div key={variable.name}>
                      <label className="text-sm font-medium block mb-1" style={{ color: colors.text }}>
                        {variable.name}
                      </label>
                      {variable.description && (
                        <p className="text-xs mb-1.5" style={{ color: colors.textMuted }}>{variable.description}</p>
                      )}
                      <input
                        type="text"
                        placeholder={variable.default_value || `Enter ${variable.name}`}
                        value={variables[variable.name] || ''}
                        onChange={(e) =>
                          setVariables({ ...variables, [variable.name]: e.target.value })
                        }
                        className="w-full px-3 py-2 rounded-lg outline-none text-sm transition-colors"
                        style={{
                          backgroundColor: colors.inputBg,
                          border: `1px solid ${colors.border}`,
                          color: colors.text,
                        }}
                        onFocus={(e) => e.target.style.borderColor = colors.primary}
                        onBlur={(e) => e.target.style.borderColor = colors.border}
                      />
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Output Format */}
            <div>
              <label className="text-xs font-semibold mb-2 block" style={{ color: colors.textMuted }}>OUTPUT FORMAT</label>
              <div className="flex gap-2 flex-wrap">
                {formatOptions.map(({ value, label }) => (
                  <button
                    key={value}
                    className="px-4 py-2 rounded-lg border text-sm font-medium transition-all"
                    style={{
                      backgroundColor: outputFormat === value ? colors.primary : 'transparent',
                      borderColor: outputFormat === value ? colors.primary : colors.border,
                      color: outputFormat === value ? colors.primaryText : colors.textMuted,
                    }}
                    onClick={() => setOutputFormat(value)}
                  >
                    {label}
                  </button>
                ))}
              </div>
            </div>

            {/* Actions */}
            <div className="flex gap-3 pt-2">
              <button
                onClick={() => setStep('library')}
                className="flex-1 py-2.5 rounded-lg font-medium flex items-center justify-center gap-2 text-sm"
                style={{
                  backgroundColor: colors.bgTertiary,
                  color: colors.text,
                  border: `1px solid ${colors.border}`,
                }}
              >
                <ArrowLeft className="w-4 h-4" />
                Back
              </button>
              <button
                onClick={handleGenerateFromTemplate}
                disabled={loading}
                className="flex-1 py-2.5 rounded-lg font-medium flex items-center justify-center gap-2 text-sm disabled:opacity-50 disabled:cursor-not-allowed"
                style={{
                  backgroundColor: colors.primary,
                  color: colors.primaryText,
                }}
              >
                {loading ? (
                  <>
                    <Loader2 className="w-4 h-4 animate-spin" />
                    Generating...
                  </>
                ) : (
                  'Generate Document'
                )}
              </button>
            </div>
          </CardContent>
        </Card>
      </motion.div>
    );
  };

  const [previewCopied, setPreviewCopied] = useState(false);

  const handleCopyPreview = () => {
    navigator.clipboard.writeText(generatedContent);
    setPreviewCopied(true);
    setTimeout(() => setPreviewCopied(false), 2000);
  };

  const renderPreview = () => (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      className="space-y-4"
    >
      <Card style={{ backgroundColor: colors.cardBg, borderColor: colors.cardBorder }}>
        <CardHeader>
          <div className="flex items-center justify-between">
            <CardTitle className="flex items-center gap-2" style={{ color: colors.text }}>
              <Check className="w-5 h-5" style={{ color: colors.success }} />
              Generated Document
            </CardTitle>
            <div className="flex gap-2">
              <button
                onClick={handleCopyPreview}
                className="px-3 py-2 rounded-lg text-sm flex items-center gap-1.5 transition-colors"
                style={{ backgroundColor: colors.bgTertiary, color: colors.text }}
              >
                {previewCopied ? <Check className="w-4 h-4" style={{ color: colors.success }} /> : <Copy className="w-4 h-4" />}
                {previewCopied ? 'Copied!' : 'Copy'}
              </button>
              <button
                onClick={() => setStep('library')}
                className="px-3 py-2 rounded-lg text-sm font-medium"
                style={{ backgroundColor: colors.primary, color: colors.primaryText }}
              >
                Done
              </button>
            </div>
          </div>
          <p className="text-sm mt-1" style={{ color: colors.textMuted }}>
            {generatedContent.length.toLocaleString()} characters &middot; {outputFormat}
          </p>
        </CardHeader>
        <CardContent>
          <div className="rounded-lg p-6 max-h-[500px] overflow-y-auto" style={{ backgroundColor: colors.bgTertiary }}>
            {(outputFormat === 'markdown' || outputFormat === 'html') ? (
              <div className="prose prose-sm dark:prose-invert max-w-none" style={{ color: colors.text }}>
                <ReactMarkdown remarkPlugins={[remarkGfm]}>{generatedContent}</ReactMarkdown>
              </div>
            ) : (
              <pre className="text-sm whitespace-pre-wrap font-mono" style={{ color: colors.text }}>
                {generatedContent}
              </pre>
            )}
          </div>
        </CardContent>
      </Card>

      <button
        onClick={() => {
          setStep('generate');
          setGeneratedContent('');
        }}
        className="w-full py-2.5 rounded-lg font-medium flex items-center justify-center gap-2 text-sm"
        style={{
          backgroundColor: colors.bgTertiary,
          color: colors.text,
          border: `1px solid ${colors.border}`,
        }}
      >
        <ArrowLeft className="w-4 h-4" />
        Back to Template
      </button>
    </motion.div>
  );

  return (
    <div className="space-y-6">
      {error && (
        <motion.div
          initial={{ opacity: 0, y: -10 }}
          animate={{ opacity: 1, y: 0 }}
          className="p-4 rounded-lg flex items-center justify-between"
          style={{
            backgroundColor: colors.error + '20',
            borderWidth: '1px',
            borderStyle: 'solid',
            borderColor: colors.error
          }}
        >
          <span style={{ color: colors.error }}>{error}</span>
          <button
            onClick={() => setError('')}
            className="px-2 py-1 rounded"
            style={{ color: colors.error }}
            onMouseEnter={(e) => e.currentTarget.style.backgroundColor = colors.error + '30'}
            onMouseLeave={(e) => e.currentTarget.style.backgroundColor = 'transparent'}
          >
            ×
          </button>
        </motion.div>
      )}

      <AnimatePresence mode="wait">
        {step === 'select' && renderSelectDocuments()}
        {step === 'library' && renderTemplateLibrary()}
        {step === 'generate' && renderGenerateDocument()}
        {step === 'preview' && renderPreview()}
      </AnimatePresence>
    </div>
  );
};

export default SmartTemplates;

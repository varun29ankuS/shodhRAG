/**
 * Artifact type normalization.
 *
 * Artifacts reach the UI in two shapes:
 * - From the backend (`shodh_rag::chat::ArtifactType`, serde `rename_all = "lowercase"`):
 *   a lowercase string tag such as `"code"` or `"chart"`. The code language, when present,
 *   is carried on the artifact's top-level `language` field.
 * - From client-side extraction (`utils/artifactExtractor`) and locally constructed previews:
 *   an externally tagged object such as `{ Code: { language } }` or `{ Chart: null }`.
 *   Unit variants carry `null`, so presence must be tested with `in`, never truthiness.
 */

export const BACKEND_ARTIFACT_TYPES = ['code', 'markdown', 'mermaid', 'table', 'chart', 'html', 'svg'] as const;

export type BackendArtifactType = typeof BACKEND_ARTIFACT_TYPES[number];

export interface ArtifactTypeVariant {
  Code?: { language: string } | null;
  Markdown?: null;
  Mermaid?: { diagram_type: string } | null;
  Chart?: null;
  Table?: null;
  SVG?: null;
  HTML?: null;
  PDF?: null;
}

export type ArtifactTypeValue = BackendArtifactType | ArtifactTypeVariant;

export type ArtifactKind = BackendArtifactType | 'pdf' | 'other';

const VARIANT_KINDS: ReadonlyArray<readonly [keyof ArtifactTypeVariant, ArtifactKind]> = [
  ['Code', 'code'],
  ['Markdown', 'markdown'],
  ['Mermaid', 'mermaid'],
  ['Chart', 'chart'],
  ['Table', 'table'],
  ['SVG', 'svg'],
  ['HTML', 'html'],
  ['PDF', 'pdf'],
];

function isBackendArtifactType(value: string): value is BackendArtifactType {
  return (BACKEND_ARTIFACT_TYPES as readonly string[]).includes(value);
}

/** Resolve either artifact type shape to a single lowercase kind. */
export function getArtifactKind(artifactType: ArtifactTypeValue | null | undefined): ArtifactKind {
  if (typeof artifactType === 'string') {
    const lower = artifactType.toLowerCase();
    return isBackendArtifactType(lower) ? lower : 'other';
  }
  if (artifactType && typeof artifactType === 'object') {
    for (const [key, kind] of VARIANT_KINDS) {
      if (key in artifactType) return kind;
    }
  }
  return 'other';
}

/** Language of a code artifact, from the tagged variant or the top-level field. */
export function getArtifactCodeLanguage(artifact: { artifact_type: ArtifactTypeValue; language?: string | null }): string | undefined {
  const t = artifact.artifact_type;
  if (typeof t === 'object' && t !== null && t.Code?.language) return t.Code.language;
  return artifact.language ?? undefined;
}

// Shared types used by both the Electron main process and the React renderer.
// No secret values ever appear in these structures.

export type Severity = 'critical' | 'high' | 'medium' | 'low' | 'info';
export type FindingStatus = 'open' | 'confirmed' | 'fixed' | 'accepted' | 'false_positive';
export type AssessmentType = 'recon' | 'vulnerability' | 'accounts' | 'rbac' | 'api' | 'php' | 'mysql' | 'full';
export type AssessmentStatus = 'pending' | 'running' | 'complete' | 'failed' | 'stopped';

export const SEVERITIES: Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

export interface Target {
  id: string;
  label: string;
  url: string;
  kind: 'web' | 'api' | 'php' | 'mysql' | 'localhost';
  authorization_note: string;
  scope_patterns: string; // JSON array of extra allowed host patterns
  rate_limit: number; // requests per second
  allow_private: number;
  created_at: string;
}

export interface Assessment {
  id: string;
  target_id: string | null;
  target_label: string;
  type: AssessmentType;
  status: AssessmentStatus;
  progress: number;
  summary: string | null;
  error: string | null;
  started_at: string;
  finished_at: string | null;
}

export interface Finding {
  id: string;
  assessment_id: string;
  target: string;
  name: string;
  category: string;
  severity: Severity;
  confidence: 'low' | 'medium' | 'high';
  endpoint: string;
  evidence: string;
  description: string;
  impact: string;
  reproduction: string;
  remediation: string;
  references: string; // JSON array
  status: FindingStatus;
  created_at: string;
}

export type FindingInput = Omit<Finding, 'id' | 'status' | 'created_at' | 'assessment_id' | 'references'> & { references: string[]; assessment_id?: string };

export interface Settings {
  aiProvider: string;
  aiModel: string;
  temperature: number;
  maxTokens: number;
  aiBaseUrl: string;
  keyConfigured: boolean;
  requiresKey: boolean;
  secureStorage: boolean;
  defaultRateLimit: number;
  requestTimeoutMs: number;
  autoApproveLevel: number; // max permission level that runs without asking (1 = passive only)
  reportsDir: string;
}

export interface ToolInfo {
  id: string;
  name: string;
  kind: 'binary' | 'builtin' | 'browser';
  installed: boolean;
  version: string | null;
  location: string | null;
  via: 'windows' | 'wsl' | 'builtin' | 'none';
  notes: string;
}

export interface Stats {
  assessments: number;
  findings: number;
  bySeverity: Record<Severity, number>;
  accountIssues: number;
  permissionIssues: number;
  recentAssessments: Assessment[];
  recentActivity: AuditEntry[];
}

export interface AuditEntry {
  id: number;
  actor: string;
  action: string;
  target: string | null;
  level: number;
  detail: string | null;
  created_at: string;
}

export interface WslStatus {
  installed: boolean;
  distributions: string[];
  ubuntu: boolean;
  notes: string;
}

export interface AccountRecord {
  username: string;
  role: string;
  active: boolean | null;
  lastLogin: string | null;
  mfa: boolean | null;
  created: string | null;
}

export interface AccountIssue {
  severity: Severity;
  name: string;
  account: string;
  detail: string;
  remediation: string;
}

export interface AccessCapture {
  id: string;
  url: string;
  title: string;
  captured_at: string;
  summary: string;
  htmlPath: string;
}

export interface ConnectionProfile {
  id: string;
  name: string;
  kind: 'rest' | 'mysql' | 'database';
  base_url: string | null;
  username: string | null;
  has_secret: number;
  created_at: string;
  config?: Record<string, unknown>; // non-secret fields only
}

export interface ApprovalRequest {
  id: string;
  level: number;
  title: string;
  detail: string;
}

export interface ChatMessage {
  id: number;
  conversation_id: string;
  role: 'user' | 'assistant' | 'tool' | 'system';
  content: string;
  created_at: string;
}

export interface Conversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
}

export interface ReportRecord {
  id: string;
  assessment_id: string;
  format: 'html' | 'pdf' | 'json';
  path: string;
  created_at: string;
}

export function severityRank(s: Severity): number {
  return { critical: 5, high: 4, medium: 3, low: 2, info: 1 }[s] ?? 0;
}

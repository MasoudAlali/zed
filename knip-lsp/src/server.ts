import {
  createConnection,
  TextDocuments,
  Diagnostic,
  DiagnosticSeverity,
  ProposedFeatures,
  InitializeParams,
  InitializeResult,
  TextDocumentSyncKind,
  CodeAction,
  CodeActionKind,
  CodeActionParams,
  TextEdit,
  Range,
  Position,
} from "vscode-languageserver/node";

import { TextDocument } from "vscode-languageserver-textdocument";
import { URI } from "vscode-uri";
import { execFile } from "child_process";
import * as path from "path";

// --- Knip JSON types ---

interface KnipIssueItem {
  name: string;
  line?: number;
  col?: number;
}

interface KnipFileIssues {
  file: string;
  files?: KnipIssueItem[];
  dependencies?: KnipIssueItem[];
  devDependencies?: KnipIssueItem[];
  unlisted?: KnipIssueItem[];
  unresolved?: KnipIssueItem[];
  binaries?: KnipIssueItem[];
  exports?: KnipIssueItem[];
  types?: KnipIssueItem[];
  duplicates?: KnipIssueItem[];
  enumMembers?: KnipIssueItem[];
  optionalPeerDependencies?: KnipIssueItem[];
  catalog?: KnipIssueItem[];
  namespaceMembers?: KnipIssueItem[];
}

interface KnipReport {
  issues?: KnipFileIssues[];
}

type IssueSeverity = "error" | "warning" | "info" | "hint";

interface IssueCategory {
  key: keyof KnipFileIssues;
  label: string;
  severity: IssueSeverity;
}

const ISSUE_CATEGORIES: IssueCategory[] = [
  { key: "files", label: "Unused file", severity: "hint" },
  { key: "dependencies", label: "Unused dependency", severity: "error" },
  { key: "devDependencies", label: "Unused devDependency", severity: "error" },
  { key: "unlisted", label: "Unlisted dependency", severity: "warning" },
  { key: "unresolved", label: "Unresolved import", severity: "warning" },
  { key: "binaries", label: "Unlisted binary", severity: "warning" },
  { key: "exports", label: "Unused export", severity: "warning" },
  { key: "types", label: "Unused type", severity: "warning" },
  { key: "duplicates", label: "Duplicate export", severity: "info" },
  { key: "enumMembers", label: "Unused enum member", severity: "info" },
  {
    key: "optionalPeerDependencies",
    label: "Referenced optional peer dependency",
    severity: "hint",
  },
  { key: "catalog", label: "Unused catalog entry", severity: "info" },
  { key: "namespaceMembers", label: "Unused namespace member", severity: "warning" },
];

// --- Server setup ---

const connection = createConnection(ProposedFeatures.all);
const documents = new TextDocuments(TextDocument);

let workspaceRoot: string | null = null;
let knipDiagnostics: Map<string, Diagnostic[]> = new Map();
let debounceTimer: ReturnType<typeof setTimeout> | undefined;

connection.onInitialize((params: InitializeParams): InitializeResult => {
  if (params.workspaceFolders && params.workspaceFolders.length > 0) {
    workspaceRoot = URI.parse(params.workspaceFolders[0].uri).fsPath;
  } else if (params.rootUri) {
    workspaceRoot = URI.parse(params.rootUri).fsPath;
  }

  return {
    capabilities: {
      textDocumentSync: TextDocumentSyncKind.Incremental,
      codeActionProvider: {
        codeActionKinds: [CodeActionKind.QuickFix],
      },
    },
  };
});

connection.onInitialized(() => {
  runKnip();
});

// --- Knip execution ---

function severityToLsp(severity: IssueSeverity): DiagnosticSeverity {
  switch (severity) {
    case "error":
      return DiagnosticSeverity.Error;
    case "warning":
      return DiagnosticSeverity.Warning;
    case "info":
      return DiagnosticSeverity.Information;
    case "hint":
      return DiagnosticSeverity.Hint;
  }
}

function makeRange(item: KnipIssueItem): Range {
  // knip lines/cols are 1-indexed; LSP is 0-indexed
  const line = item.line ? item.line - 1 : 0;
  const col = item.col ? item.col - 1 : 0;
  return {
    start: Position.create(line, col),
    end: Position.create(line, col + item.name.length),
  };
}

function runKnip(): void {
  if (!workspaceRoot) {
    return;
  }

  connection.console.log("knip-lsp: running knip...");

  const npxPath = process.platform === "win32" ? "npx.cmd" : "npx";

  execFile(
    npxPath,
    ["knip", "--reporter", "json", "--no-progress"],
    {
      cwd: workspaceRoot,
      maxBuffer: 10 * 1024 * 1024, // 10 MB
      timeout: 120_000,
    },
    (error, stdout, stderr) => {
      // knip exits with code 1 when issues are found — that's normal
      if (error && error.code !== 1 && !stdout) {
        connection.console.error(`knip-lsp: knip failed — ${stderr || error.message}`);
        return;
      }

      const trimmed = stdout.trim();
      if (!trimmed) {
        clearAllDiagnostics();
        return;
      }

      let report: KnipReport;
      try {
        report = JSON.parse(trimmed);
      } catch (parseError) {
        connection.console.error(`knip-lsp: failed to parse knip output — ${parseError}`);
        return;
      }

      publishDiagnostics(report);
    }
  );
}

function publishDiagnostics(report: KnipReport): void {
  // Clear previous diagnostics for files that are no longer in the report
  const newFiles = new Set<string>();

  const diagnosticsByFile = new Map<string, Diagnostic[]>();

  for (const fileIssues of report.issues ?? []) {
    const filePath = path.isAbsolute(fileIssues.file)
      ? fileIssues.file
      : path.join(workspaceRoot!, fileIssues.file);
    const fileUri = URI.file(filePath).toString();
    newFiles.add(fileUri);

    const diagnostics: Diagnostic[] = diagnosticsByFile.get(fileUri) ?? [];

    for (const category of ISSUE_CATEGORIES) {
      const items = fileIssues[category.key] as KnipIssueItem[] | undefined;
      if (!items) continue;

      for (const item of items) {
        diagnostics.push({
          range: makeRange(item),
          severity: severityToLsp(category.severity),
          source: "knip",
          message: `${category.label}: '${item.name}'`,
          code: category.key as string,
        });
      }
    }

    diagnosticsByFile.set(fileUri, diagnostics);
  }

  // Clear diagnostics for files no longer in report
  for (const [uri] of knipDiagnostics) {
    if (!newFiles.has(uri)) {
      connection.sendDiagnostics({ uri, diagnostics: [] });
    }
  }

  // Send new diagnostics
  for (const [uri, diagnostics] of diagnosticsByFile) {
    connection.sendDiagnostics({ uri, diagnostics });
  }

  knipDiagnostics = diagnosticsByFile;
  connection.console.log(
    `knip-lsp: published ${[...diagnosticsByFile.values()].reduce((a, d) => a + d.length, 0)} diagnostics across ${diagnosticsByFile.size} files`
  );
}

function clearAllDiagnostics(): void {
  for (const [uri] of knipDiagnostics) {
    connection.sendDiagnostics({ uri, diagnostics: [] });
  }
  knipDiagnostics.clear();
  connection.console.log("knip-lsp: no issues found, cleared diagnostics");
}

// --- Re-run on file changes ---

documents.onDidSave(() => {
  scheduleKnipRun();
});

documents.onDidClose((event) => {
  // Clear diagnostics when a file is closed
  const uri = event.document.uri;
  if (knipDiagnostics.has(uri)) {
    connection.sendDiagnostics({ uri, diagnostics: [] });
    knipDiagnostics.delete(uri);
  }
});

function scheduleKnipRun(): void {
  if (debounceTimer) {
    clearTimeout(debounceTimer);
  }
  // Debounce: wait 2 seconds after last save before re-running
  debounceTimer = setTimeout(() => {
    runKnip();
  }, 2000);
}

// --- Code actions ---

connection.onCodeAction((params: CodeActionParams): CodeAction[] => {
  const actions: CodeAction[] = [];
  const uri = params.textDocument.uri;
  const fileDiagnostics = knipDiagnostics.get(uri) ?? [];

  for (const diagnostic of params.context.diagnostics) {
    if (diagnostic.source !== "knip") continue;

    // Match against our stored diagnostics
    const matched = fileDiagnostics.find(
      (d) =>
        d.range.start.line === diagnostic.range.start.line &&
        d.range.start.character === diagnostic.range.start.character &&
        d.code === diagnostic.code
    );

    if (!matched) continue;

    const code = matched.code as string;

    // Offer "Remove export" for unused exports
    if (code === "exports" || code === "types") {
      const document = documents.get(uri);
      if (document) {
        const line = document.getText({
          start: Position.create(matched.range.start.line, 0),
          end: Position.create(matched.range.start.line + 1, 0),
        });

        // Simple heuristic: if line starts with "export ", offer to remove the keyword
        if (line.trimStart().startsWith("export ")) {
          const exportStart = line.indexOf("export ");
          actions.push({
            title: `Remove 'export' from '${(matched.message.match(/'([^']+)'/) ?? [])[1] ?? "item"}'`,
            kind: CodeActionKind.QuickFix,
            diagnostics: [diagnostic],
            edit: {
              changes: {
                [uri]: [
                  TextEdit.replace(
                    Range.create(
                      Position.create(matched.range.start.line, exportStart),
                      Position.create(matched.range.start.line, exportStart + 7) // "export " = 7 chars
                    ),
                    ""
                  ),
                ],
              },
            },
          });
        }
      }
    }
  }

  return actions;
});

// --- Start ---

documents.listen(connection);
connection.listen();

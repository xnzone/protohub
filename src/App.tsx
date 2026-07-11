import Editor, { type BeforeMount, type OnMount } from "@monaco-editor/react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowRight,
  Braces,
  Check,
  ChevronDown,
  ChevronRight,
  CircleDot,
  FileCode2,
  FolderOpen,
  GitBranch,
  Hash,
  List,
  MessageSquare,
  Play,
  RefreshCcw,
  Save,
  Search,
  Server,
  Settings2,
  SplitSquareHorizontal,
  Variable,
  X,
  Plus,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from "react";

type ViewMode = "editor" | "tester";

type FileNode = {
  name: string;
  path: string;
  isDir: boolean;
  children: FileNode[];
};

type ProtoMethod = {
  name: string;
  requestType: string;
  responseType: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
  line: number;
};

type ProtoService = {
  name: string;
  fullName: string;
  line: number;
  methods: ProtoMethod[];
};

type ProtoSymbol = {
  name: string;
  kind: string;
  line: number;
};

type ProtoAnalysis = {
  packageName: string;
  services: ProtoService[];
  symbols: ProtoSymbol[];
  descriptorAvailable: boolean;
  diagnostics: string[];
};

type FieldInfo = {
  name: string;
  jsonName: string;
  fieldType: string;
  repeated: boolean;
  map: boolean;
  required: boolean;
  children: FieldInfo[];
};

type MethodTemplate = {
  requestType: string;
  responseType: string;
  requestJson: unknown;
  requestFields: FieldInfo[];
  grpcPath: string;
  clientStreaming: boolean;
  serverStreaming: boolean;
};

type DefinitionLocation = {
  path: string;
  line: number;
};

type GitWorkspaceInfo = {
  repositoryRoot: string;
  remoteUrl: string;
  currentBranch: string;
  availableBranches: string[];
  defaultBranch: string;
};

type GitPullConfig = {
  branch: string;
  intervalMinutes: number;
};

type GitChange = {
  path: string;
  relativePath: string;
  status: string;
};

const GIT_PULL_STORAGE_KEY = "protohub.gitPullConfigs";

function loadGitPullConfig(
  root: string,
  availableBranches: string[],
  fallbackBranch: string,
): GitPullConfig {
  try {
    const configs = JSON.parse(localStorage.getItem(GIT_PULL_STORAGE_KEY) || "{}") as Record<
      string,
      GitPullConfig
    >;
    const saved = configs[root];
    if (
      saved &&
      availableBranches.includes(saved.branch) &&
      [0, 1, 5, 15, 30, 60].includes(saved.intervalMinutes)
    ) return saved;
  } catch {
    // Use the repository defaults when storage is unavailable or invalid.
  }
  return { branch: fallbackBranch, intervalMinutes: 0 };
}

function saveGitPullConfig(root: string, config: GitPullConfig): void {
  try {
    const configs = JSON.parse(localStorage.getItem(GIT_PULL_STORAGE_KEY) || "{}") as Record<
      string,
      GitPullConfig
    >;
    configs[root] = config;
    localStorage.setItem(GIT_PULL_STORAGE_KEY, JSON.stringify(configs));
  } catch {
    // The current session can still use the selected configuration.
  }
}

const emptyAnalysis: ProtoAnalysis = {
  packageName: "",
  services: [],
  symbols: [],
  descriptorAvailable: false,
  diagnostics: [],
};

type EnvVariable = {
  key: string;
  value: string;
};

const LEGACY_DEFAULT_ENV_VARS: EnvVariable[] = [
  { key: "LOCAL_HOST", value: "127.0.0.1:8000" },
  { key: "WEIYIN_LOCAL", value: "127.0.0.1:8001" },
  { key: "TEST_HOST", value: "10.17.8.17:65501" },
  { key: "WEIYIN_TEST", value: "10.240.2.18:65501" },
  { key: "STAGING_HOST", value: "10.16.2.146:65001" },
  { key: "PROD_HOST", value: "10.16.2.147:4700" },
];

const ENV_VARS_STORAGE_KEY = "protohub.envVars";

function loadEnvVars(): EnvVariable[] {
  try {
    const raw = localStorage.getItem(ENV_VARS_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as EnvVariable[];
    if (!Array.isArray(parsed)) return [];

    // Remove the project-specific defaults previously shipped with ProtoHub,
    // while preserving any entries whose key or value the user customized.
    return parsed.filter(
      (item) =>
        !LEGACY_DEFAULT_ENV_VARS.some(
          (legacy) => legacy.key === item.key && legacy.value === item.value,
        ),
    );
  } catch {
    return [];
  }
}

function saveEnvVars(vars: EnvVariable[]): void {
  try {
    localStorage.setItem(ENV_VARS_STORAGE_KEY, JSON.stringify(vars));
  } catch {
    // Storage unavailable; the in-memory values still work for this session.
  }
}

function expandTemplate(template: string, vars: EnvVariable[]): string {
  if (!vars.length) return template;
  const lookup = new Map(vars.map((item) => [item.key, item.value]));
  return template.replace(/\{\{\s*([\w.-]+)\s*\}\}/g, (match, key: string) => {
    const value = lookup.get(key);
    return value !== undefined ? value : match;
  });
}

const sampleProto = `syntax = "proto3";

package demo.v1;

service Greeter {
  rpc SayHello (HelloRequest) returns (HelloReply);
}

message HelloRequest {
  string name = 1;
}

message HelloReply {
  string message = 1;
}
`;

export default function App() {
  const [view, setView] = useState<ViewMode>("editor");
  const [rootPath, setRootPath] = useState("");
  const [tree, setTree] = useState<FileNode | null>(null);
  const [activeFile, setActiveFile] = useState("");
  const [content, setContent] = useState(sampleProto);
  const [savedContent, setSavedContent] = useState(sampleProto);
  const [analysis, setAnalysis] = useState<ProtoAnalysis>(emptyAnalysis);
  const [selectedService, setSelectedService] = useState("");
  const [selectedMethod, setSelectedMethod] = useState("");
  const [endpoint, setEndpoint] = useState("127.0.0.1:8000");
  const [useTls, setUseTls] = useState(false);
  const [envVars, setEnvVars] = useState<EnvVariable[]>(loadEnvVars);
  const [envPanelOpen, setEnvPanelOpen] = useState(false);
  const [metadata, setMetadata] = useState("authorization: Bearer token");
  const [requestJson, setRequestJson] = useState("{\n  \"name\": \"ProtoHub\"\n}");
  const [responseJson, setResponseJson] = useState("");
  const [requestFields, setRequestFields] = useState<FieldInfo[]>([]);
  const [grpcPath, setGrpcPath] = useState("");
  const [fullMethod, setFullMethod] = useState("");
  const [authority, setAuthority] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [isBusy, setIsBusy] = useState(false);
  const [status, setStatus] = useState("Ready");
  const [gitInfo, setGitInfo] = useState<GitWorkspaceInfo | null>(null);
  const [gitConfig, setGitConfig] = useState<GitPullConfig>({ branch: "main", intervalMinutes: 0 });
  const [gitPanelOpen, setGitPanelOpen] = useState(false);
  const [isPulling, setIsPulling] = useState(false);
  const [gitChanges, setGitChanges] = useState<GitChange[]>([]);
  const isPullingRef = useRef(false);
  const [activeLine, setActiveLine] = useState<number | null>(null);
  const editorRef = useRef<Parameters<OnMount>[0] | null>(null);
  const analysisRef = useRef<ProtoAnalysis>(emptyAnalysis);
  const activeFileRef = useRef("");
  const contentRef = useRef(sampleProto);
  const includePathsRef = useRef<string[]>([]);
  const highlightDecorationsRef = useRef<string[]>([]);
  const templateKeyRef = useRef("");

  const isDirty = content !== savedContent;
  const selectedServiceData = analysis.services.find(
    (service) => service.fullName === selectedService || service.name === selectedService,
  );
  const selectedMethodData = selectedServiceData?.methods.find(
    (method) => method.name === selectedMethod,
  );

  useEffect(() => {
    if (!selectedServiceData) return;
    const servicePackage = selectedServiceData.fullName.includes(".")
      ? selectedServiceData.fullName.substring(0, selectedServiceData.fullName.lastIndexOf("."))
      : "";
    setAuthority(servicePackage);
  }, [selectedServiceData]);
  const includePaths = useMemo(() => {
    const paths = new Set<string>();
    if (rootPath) paths.add(rootPath);
    if (activeFile.includes("/")) paths.add(activeFile.split("/").slice(0, -1).join("/"));
    return Array.from(paths);
  }, [activeFile, rootPath]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      analyzeCurrentProto();
    }, 220);
    return () => window.clearTimeout(timer);
  }, [activeFile, content]);

  useEffect(() => {
    if (!selectedService && analysis.services[0]) {
      setSelectedService(analysis.services[0].fullName);
    }
  }, [analysis.services, selectedService]);

  useEffect(() => {
    const service = analysis.services.find((item) => item.fullName === selectedService);
    if (service && !service.methods.some((method) => method.name === selectedMethod)) {
      setSelectedMethod(service.methods[0]?.name ?? "");
    }
  }, [analysis.services, selectedMethod, selectedService]);

  useEffect(() => {
    loadMethodTemplate();
  }, [activeFile, selectedService, selectedMethod, includePaths.join("\n")]);

  useEffect(() => {
    analysisRef.current = analysis;
  }, [analysis]);

  useEffect(() => {
    saveEnvVars(envVars);
  }, [envVars]);

  useEffect(() => {
    activeFileRef.current = activeFile;
  }, [activeFile]);

  useEffect(() => {
    contentRef.current = content;
  }, [content]);

  useEffect(() => {
    includePathsRef.current = includePaths;
  }, [includePaths]);

  useEffect(() => {
    if (!rootPath) {
      setGitInfo(null);
      return;
    }
    void invoke<GitWorkspaceInfo | null>("get_git_workspace_info", { root: rootPath })
      .then((info) => {
        setGitInfo(info);
        if (info) {
          setGitConfig(
            loadGitPullConfig(info.repositoryRoot, info.availableBranches, info.defaultBranch),
          );
          void loadGitChanges(info.repositoryRoot);
        } else {
          setGitPanelOpen(false);
          setGitChanges([]);
        }
      })
      .catch(() => setGitInfo(null));
  }, [rootPath]);

  useEffect(() => {
    if (!gitInfo || gitConfig.intervalMinutes <= 0) return;
    const timer = window.setInterval(
      () => void pullFromRemote(true),
      gitConfig.intervalMinutes * 60_000,
    );
    return () => window.clearInterval(timer);
  }, [gitInfo?.repositoryRoot, gitConfig.branch, gitConfig.intervalMinutes, activeFile, isDirty]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "s") return;
      event.preventDefault();
      void saveFile();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [activeFile, content]);

  async function chooseWorkspace() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected !== "string") return;
    setRootPath(selected);
    setStatus("Loading workspace");
    const nextTree = await invoke<FileNode>("read_workspace", { root: selected });
    setTree(nextTree);
    const firstProto = findFirstProto(nextTree);
    if (firstProto) {
      await openFile(firstProto.path);
    } else {
      setStatus("Workspace loaded");
    }
  }

  async function refreshWorkspace() {
    if (!rootPath) return;
    const nextTree = await invoke<FileNode>("read_workspace", { root: rootPath });
    setTree(nextTree);
    if (gitInfo) await loadGitChanges(gitInfo.repositoryRoot);
    setStatus("Workspace refreshed");
  }

  async function loadGitChanges(repositoryRoot: string) {
    try {
      setGitChanges(await invoke<GitChange[]>("get_git_changes", { root: repositoryRoot }));
    } catch {
      setGitChanges([]);
    }
  }

  function updateGitConfig(next: GitPullConfig) {
    setGitConfig(next);
    if (gitInfo) saveGitPullConfig(gitInfo.repositoryRoot, next);
  }

  async function pullFromRemote(automatic = false) {
    if (!gitInfo || isPullingRef.current) return;
    isPullingRef.current = true;
    setIsPulling(true);
    setStatus(`${automatic ? "Auto-pulling" : "Pulling"} origin/${gitConfig.branch}`);
    try {
      const result = await invoke<{ message: string }>("pull_git_branch", {
        root: gitInfo.repositoryRoot,
        branch: gitConfig.branch,
      });
      const nextTree = await invoke<FileNode>("read_workspace", { root: rootPath });
      setTree(nextTree);
      await loadGitChanges(gitInfo.repositoryRoot);
      if (activeFile && !isDirty) {
        const nextContent = await invoke<string>("read_text_file", { path: activeFile });
        setContent(nextContent);
        setSavedContent(nextContent);
      }
      setStatus(`origin/${gitConfig.branch}: ${result.message}`);
    } catch (error) {
      setStatus(`Git pull failed: ${String(error)}`);
    } finally {
      isPullingRef.current = false;
      setIsPulling(false);
    }
  }

  async function openFile(path: string) {
    const nextContent = await invoke<string>("read_text_file", { path });
    setActiveFile(path);
    setContent(nextContent);
    setSavedContent(nextContent);
    setStatus(`Opened ${fileName(path)}`);
  }

  async function saveFile() {
    if (!activeFile) return;
    await invoke("write_text_file", { path: activeFile, content });
    setSavedContent(content);
    if (gitInfo) await loadGitChanges(gitInfo.repositoryRoot);
    setStatus(`Saved ${fileName(activeFile)}`);
  }

  async function analyzeCurrentProto() {
    try {
      const result = await invoke<ProtoAnalysis>("analyze_proto", {
        input: {
          filePath: activeFile || "scratch.proto",
          content,
          includePaths,
        },
      });
      setAnalysis(result);
    } catch (error) {
      const fallback = parseProtoClientSide(content, activeFile || "scratch.proto");
      if (fallback) {
        setAnalysis(fallback);
      } else {
        setAnalysis({ ...emptyAnalysis, diagnostics: [String(error)] });
      }
    }
  }

  async function sendGrpcRequest() {
    if (!activeFile || !selectedService || !selectedMethod) {
      setStatus("Open a proto file and select a method first");
      return;
    }

    setIsBusy(true);
    setResponseJson("");
    setStatus("Sending request");
    try {
      const parsedRequest = JSON.parse(requestJson || "{}");
      const endpointValue = expandTemplate(endpoint.trim(), envVars);
      if (!endpointValue) {
        throw new Error("Endpoint is empty (no variable resolved)");
      }
      const result = await invoke<{
        status: string;
        responseJson: unknown;
        elapsedMs: number;
        grpcPath: string;
      }>(
        "invoke_grpc",
        {
          input: {
            endpoint: endpointValue,
            filePath: activeFile,
            includePaths,
            service: selectedService,
            method: selectedMethod,
            grpcPath: fullMethod || grpcPath,
            authority: authority.trim(),
            requestJson: parsedRequest,
            metadata: parseMetadata(metadata),
            useTls,
          },
        },
      );
      setResponseJson(JSON.stringify(result.responseJson, null, 2));
      setGrpcPath(result.grpcPath);
      setStatus(`${result.status} · ${result.elapsedMs} ms · ${result.grpcPath}`);
    } catch (error) {
      setResponseJson(JSON.stringify({ error: String(error) }, null, 2));
      setStatus("Request failed");
    } finally {
      setIsBusy(false);
    }
  }

  async function loadMethodTemplate() {
    if (!activeFile || !selectedService || !selectedMethod) return;
    const key = `${activeFile}|${selectedService}|${selectedMethod}`;
    if (templateKeyRef.current === key) return;

    try {
      const template = await invoke<MethodTemplate>("get_method_template", {
        input: {
          filePath: activeFile,
          includePaths,
          service: selectedService,
          method: selectedMethod,
        },
      });
      templateKeyRef.current = key;
      setGrpcPath(template.grpcPath);
      setFullMethod(template.grpcPath);
      setRequestFields(template.requestFields);
      setRequestJson(JSON.stringify(template.requestJson, null, 2));
      setStatus(`Template loaded · ${template.grpcPath}`);
    } catch (error) {
      setRequestFields([]);
      setGrpcPath("");
      setStatus(`Template unavailable: ${String(error)}`);
    }
  }

  const handleEditorMount: OnMount = (editor) => {
    editorRef.current = editor;
    editor.onMouseDown(async (event) => {
      if (!event.event.metaKey || !event.target.position) return;
      const model = editor.getModel();
      if (!model) return;
      const word = model.getWordAtPosition(event.target.position);
      if (!word?.word) return;
      event.event.preventDefault();
      const location = await findDefinitionLocation(word.word);
      if (!location) return;
      await jumpToLocation(location);
    });
  };

  async function findDefinitionLocation(word: string): Promise<DefinitionLocation | undefined> {
    const currentFile = activeFileRef.current;
    const currentLine = findCurrentFileDefinitionLine(word);
    if (currentLine && currentFile) {
      return { path: currentFile, line: currentLine };
    }

    const cleanWord = word.replace(/^\./, "");
    const location = await findDefinitionInImports(
      currentFile,
      contentRef.current,
      cleanWord,
      new Set(),
    );
    if (location) return location;
    setStatus(`Definition not found: ${cleanWord}`);
    return undefined;
  }

  async function findDefinitionInImports(
    sourcePath: string,
    sourceContent: string,
    word: string,
    visited: Set<string>,
  ): Promise<DefinitionLocation | undefined> {
    for (const importPath of parseProtoImports(sourceContent)) {
      const resolvedPath = await resolveImportPath(importPath, sourcePath);
      if (!resolvedPath || visited.has(resolvedPath)) continue;
      visited.add(resolvedPath);

      try {
        const importedContent = await invoke<string>("read_text_file", { path: resolvedPath });
        const line = findDefinitionInContent(importedContent, word);
        if (line) return { path: resolvedPath, line };

        const nested = await findDefinitionInImports(
          resolvedPath,
          importedContent,
          word,
          visited,
        );
        if (nested) return nested;
      } catch {
        // Ignore unreadable imports and continue through the remaining import graph.
      }
    }
    return undefined;
  }

  async function resolveImportPath(importPath: string, sourcePath: string) {
    const sourceDir = sourcePath.includes("/") ? sourcePath.split("/").slice(0, -1).join("/") : "";
    const candidates = [
      sourceDir ? `${sourceDir}/${importPath}` : importPath,
      ...includePathsRef.current.map(
        (includePath) => `${includePath.replace(/\/$/, "")}/${importPath}`,
      ),
    ];

    for (const candidate of Array.from(new Set(candidates))) {
      try {
        await invoke<string>("read_text_file", { path: candidate });
        return candidate;
      } catch {
        // Try the next include root.
      }
    }
    return undefined;
  }

  function findCurrentFileDefinitionLine(word: string) {
    const current = analysisRef.current;
    const cleanWord = word.replace(/^\./, "");
    const symbol = current.symbols.find(
      (item) => item.name === cleanWord || item.name.endsWith(`.${cleanWord}`),
    );
    if (symbol) return symbol.line;

    for (const service of current.services) {
      if (service.name === cleanWord || service.fullName === cleanWord) return service.line;
      const method = service.methods.find((item) => item.name === cleanWord);
      if (method) return method.line;
    }

    return undefined;
  }

  async function jumpToLocation(location: DefinitionLocation) {
    if (location.path && location.path !== activeFileRef.current) {
      await openFile(location.path);
      setStatus(`Jumped to ${fileName(location.path)}:${location.line}`);
    }
    jumpToLine(location.line);
  }

  function jumpToLine(line: number) {
    setActiveLine(line);
    setView("editor");
    window.requestAnimationFrame(() => {
      const editor = editorRef.current;
      if (!editor) return;
      editor.revealLineInCenter(line);
      editor.setPosition({ lineNumber: line, column: 1 });
      editor.setSelection({
        startLineNumber: line,
        startColumn: 1,
        endLineNumber: line,
        endColumn: 1,
      });
      highlightDecorationsRef.current = editor.deltaDecorations(highlightDecorationsRef.current, [
        {
          range: {
            startLineNumber: line,
            startColumn: 1,
            endLineNumber: line,
            endColumn: 1,
          },
          options: {
            isWholeLine: true,
            className: "jump-line-highlight",
            glyphMarginClassName: "jump-line-glyph",
          },
        },
      ]);
      window.setTimeout(() => {
        const currentEditor = editorRef.current;
        if (!currentEditor) return;
        highlightDecorationsRef.current = currentEditor.deltaDecorations(
          highlightDecorationsRef.current,
          [],
        );
      }, 1300);
      editor.focus();
    });
  }

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">
            <Braces size={19} />
          </div>
          <div>
            <div className="brand-name">ProtoHub</div>
            <div className="brand-subtitle">protobuf lab</div>
          </div>
        </div>

        <div className="sidebar-actions">
          <button className="primary-button" onClick={chooseWorkspace}>
            <FolderOpen size={16} />
            Open
          </button>
          <button className="icon-button" onClick={refreshWorkspace} title="Refresh workspace">
            <RefreshCcw size={16} />
          </button>
          {gitInfo && (
            <button
              className={`icon-button ${gitPanelOpen ? "active" : ""}`}
              onClick={() => setGitPanelOpen((value) => !value)}
              title="Git auto-pull settings"
            >
              <GitBranch size={16} />
              {gitChanges.length > 0 && <span className="git-change-count">{gitChanges.length}</span>}
            </button>
          )}
        </div>

        <div className="search-box">
          <Search size={15} />
          <span>{rootPath ? fileName(rootPath) : "No workspace"}</span>
        </div>

        {gitInfo && gitPanelOpen && (
          <section className="git-settings">
            <div className="git-settings-title">
              <GitBranch size={14} />
              <span>Auto-pull</span>
            </div>
            <div className="git-remote" title={gitInfo.remoteUrl}>
              {gitInfo.remoteUrl}
            </div>
            <div className="git-changes-header">
              <span>Changes</span>
              <button
                className="git-refresh-button"
                onClick={() => void loadGitChanges(gitInfo.repositoryRoot)}
                title="Refresh changes"
              >
                <RefreshCcw size={12} />
              </button>
            </div>
            <div className="git-changes">
              {gitChanges.length ? (
                gitChanges.map((change) => {
                  const canOpen = !change.status.includes("D") && change.path.endsWith(".proto");
                  return (
                    <button
                      className="git-change-row"
                      key={`${change.status}-${change.relativePath}`}
                      onClick={() => canOpen && void openFile(change.path)}
                      disabled={!canOpen}
                      title={change.relativePath}
                    >
                      <span className={`git-change-status status-${change.status.replace(/\W/g, "u")}`}>
                        {gitStatusLabel(change.status)}
                      </span>
                      <span>{change.relativePath}</span>
                    </button>
                  );
                })
              ) : (
                <div className="git-clean">No local changes</div>
              )}
            </div>
            <label>
              Branch
              <select
                value={gitConfig.branch}
                onChange={(event) => updateGitConfig({ ...gitConfig, branch: event.target.value })}
              >
                {gitInfo.availableBranches.map((branch) => <option key={branch}>{branch}</option>)}
              </select>
            </label>
            <label>
              Interval
              <select
                value={gitConfig.intervalMinutes}
                onChange={(event) =>
                  updateGitConfig({
                    ...gitConfig,
                    intervalMinutes: Number(event.target.value),
                  })
                }
              >
                <option value={0}>Off</option>
                <option value={1}>Every minute</option>
                <option value={5}>Every 5 minutes</option>
                <option value={15}>Every 15 minutes</option>
                <option value={30}>Every 30 minutes</option>
                <option value={60}>Every hour</option>
              </select>
            </label>
            <button
              className="secondary-button git-pull-button"
              onClick={() => void pullFromRemote()}
              disabled={isPulling}
            >
              <RefreshCcw size={14} className={isPulling ? "spin" : ""} />
              {isPulling ? "Pulling…" : "Pull now"}
            </button>
          </section>
        )}

        <nav className="file-tree">
          {tree ? (
            <TreeNode activeFile={activeFile} node={tree} onOpen={openFile} depth={0} />
          ) : (
            <div className="empty-panel">
              <FileCode2 size={24} />
              <span>Open a folder with .proto files.</span>
            </div>
          )}
        </nav>
      </aside>

      <section className="workspace">
        <header className="topbar">
          <div className="tabs">
            <button className={view === "editor" ? "active" : ""} onClick={() => setView("editor")}>
              <SplitSquareHorizontal size={16} />
              Editor
            </button>
            <button className={view === "tester" ? "active" : ""} onClick={() => setView("tester")}>
              <Server size={16} />
              Test
            </button>
          </div>
          <div className="file-title">
            <FileCode2 size={16} />
            <span>{activeFile ? fileName(activeFile) : "scratch.proto"}</span>
            {isDirty && <CircleDot size={12} className="dirty-dot" />}
          </div>
          <div className="topbar-actions">
            <span className={analysis.descriptorAvailable ? "pill ok" : "pill warn"}>
              {analysis.descriptorAvailable ? "descriptor" : "light parse"}
            </span>
            <button className="save-button" onClick={saveFile} disabled={!activeFile || !isDirty} title="Save">
              <Save size={16} />
              <span>Save</span>
            </button>
          </div>
        </header>

        {view === "editor" ? (
          <EditorView
            activeLine={activeLine}
            analysis={analysis}
            content={content}
            setContent={setContent}
            activeFile={activeFile}
            onEditorMount={handleEditorMount}
            onBeforeMount={configureProtoEditor}
            onJump={jumpToLine}
          />
        ) : (
          <TesterView
            analysis={analysis}
            endpoint={endpoint}
            setEndpoint={setEndpoint}
            useTls={useTls}
            setUseTls={setUseTls}
            metadata={metadata}
            setMetadata={setMetadata}
            requestJson={requestJson}
            setRequestJson={setRequestJson}
            responseJson={responseJson}
            selectedService={selectedService}
            setSelectedService={setSelectedService}
            selectedMethod={selectedMethod}
            setSelectedMethod={setSelectedMethod}
            selectedMethodData={selectedMethodData}
            requestFields={requestFields}
            grpcPath={grpcPath}
            fullMethod={fullMethod}
            setFullMethod={setFullMethod}
            authority={authority}
            setAuthority={setAuthority}
            advancedOpen={advancedOpen}
            setAdvancedOpen={setAdvancedOpen}
            envVars={envVars}
            setEnvVars={setEnvVars}
            envPanelOpen={envPanelOpen}
            setEnvPanelOpen={setEnvPanelOpen}
            sendGrpcRequest={sendGrpcRequest}
            isBusy={isBusy}
          />
        )}

        <footer className="statusbar">
          <span>{status}</span>
          <span>{analysis.packageName || "no package"}</span>
        </footer>
      </section>
    </main>
  );
}

function EditorView({
  activeLine,
  analysis,
  content,
  setContent,
  activeFile,
  onEditorMount,
  onBeforeMount,
  onJump,
}: {
  activeLine: number | null;
  analysis: ProtoAnalysis;
  content: string;
  setContent: (value: string) => void;
  activeFile: string;
  onEditorMount: OnMount;
  onBeforeMount: BeforeMount;
  onJump: (line: number) => void;
}) {
  const messageSymbols = analysis.symbols.filter((symbol) => symbol.kind !== "service");

  return (
    <div className="editor-grid">
      <section className="editor-pane">
        <Editor
          height="100%"
          language="protobuf"
          theme="protohub-light"
          path={activeFile || "scratch.proto"}
          value={content}
          beforeMount={onBeforeMount}
          onMount={onEditorMount}
          onChange={(value) => setContent(value ?? "")}
          options={{
            minimap: { enabled: false },
            fontSize: 14,
            lineHeight: 22,
            fontLigatures: true,
            scrollBeyondLastLine: false,
            automaticLayout: true,
            tabSize: 2,
          }}
        />
      </section>
      <aside className="outline">
        <div className="panel-header">
          <Settings2 size={16} />
          <span>Navigate</span>
        </div>
        <div className="outline-group">
          <div className="outline-section">
            <div className="outline-section-title">
              <Server size={13} />
              Services
            </div>
            {analysis.services.length ? (
              analysis.services.map((service) => (
                <div className="outline-service" key={service.fullName}>
                  <button
                    className={`outline-title ${activeLine === service.line ? "active" : ""}`}
                    onClick={() => onJump(service.line)}
                    title={`${service.fullName} (line ${service.line})`}
                  >
                    <span className="outline-title-main">
                      <Server size={14} className="outline-icon service" />
                      <span>{service.name}</span>
                    </span>
                    <span className="outline-line">line {service.line}</span>
                  </button>
                  {service.methods.map((method) => (
                    <button
                      className={`outline-method ${activeLine === method.line ? "active" : ""}`}
                      key={method.name}
                      onClick={() => onJump(method.line)}
                      title={`${method.name} (${method.requestType} → ${method.responseType})`}
                    >
                      <span className="outline-method-main">
                        <ArrowRight size={12} className="outline-icon method" />
                        <span>{method.name}</span>
                      </span>
                      <small>
                        {method.requestType} → {method.responseType}
                      </small>
                    </button>
                  ))}
                </div>
              ))
            ) : (
              <div className="outline-empty">No service in this file.</div>
            )}
          </div>

          <div className="outline-section">
            <div className="outline-section-title">
              <MessageSquare size={13} />
              Messages & Enums
            </div>
            {messageSymbols.length ? (
              messageSymbols.map((symbol) => {
                const Icon = symbol.kind === "enum" ? List : MessageSquare;
                const iconClass = symbol.kind === "enum" ? "enum" : "message";
                return (
                  <button
                    className={`symbol-row ${activeLine === symbol.line ? "active" : ""}`}
                    key={`${symbol.kind}-${symbol.name}-${symbol.line}`}
                    onClick={() => onJump(symbol.line)}
                    title={`${symbol.name} (${symbol.kind}, line ${symbol.line})`}
                  >
                    <span className="symbol-main">
                      <Icon size={13} className={`outline-icon ${iconClass}`} />
                      <span>{symbol.name}</span>
                    </span>
                    <span className="symbol-meta">
                      <span className="symbol-kind">{symbol.kind}</span>
                      <span className="outline-line">line {symbol.line}</span>
                    </span>
                  </button>
                );
              })
            ) : (
              <div className="outline-empty">No message or enum in this file.</div>
            )}
          </div>
        </div>
        <div className="diagnostics">
          {analysis.diagnostics.slice(-3).map((item) => (
            <div key={item}>{item}</div>
          ))}
        </div>
      </aside>
    </div>
  );
}

function TesterView(props: {
  analysis: ProtoAnalysis;
  endpoint: string;
  setEndpoint: (value: string) => void;
  useTls: boolean;
  setUseTls: (value: boolean) => void;
  metadata: string;
  setMetadata: (value: string) => void;
  requestJson: string;
  setRequestJson: (value: string) => void;
  responseJson: string;
  selectedService: string;
  setSelectedService: (value: string) => void;
  selectedMethod: string;
  setSelectedMethod: (value: string) => void;
  selectedMethodData?: ProtoMethod;
  requestFields: FieldInfo[];
  grpcPath: string;
  fullMethod: string;
  setFullMethod: (value: string) => void;
  authority: string;
  setAuthority: (value: string) => void;
  advancedOpen: boolean;
  setAdvancedOpen: (value: boolean) => void;
  envVars: EnvVariable[];
  setEnvVars: (vars: EnvVariable[]) => void;
  envPanelOpen: boolean;
  setEnvPanelOpen: (value: boolean) => void;
  sendGrpcRequest: () => void;
  isBusy: boolean;
}) {
  const service = props.analysis.services.find(
    (item) => item.fullName === props.selectedService,
  );
  const serviceOptions = props.analysis.services.map((item) => ({
    value: item.fullName,
    label: item.fullName,
  }));
  const methodOptions = (service?.methods ?? []).map((method) => ({
    value: method.name,
    label: method.name,
  }));
  return (
    <div className="tester-grid">
      <section className="request-panel">
        <div className="form-strip endpoint-strip">
          <label className="endpoint-label">
            <span>Endpoint</span>
            <div className="endpoint-input-wrap">
              <input
                className="endpoint-input"
                value={props.endpoint}
                onChange={(event) => props.setEndpoint(event.target.value)}
                placeholder="127.0.0.1:8000 或 {{LOCAL_HOST}}"
                spellCheck={false}
              />
              <div className="endpoint-var-select">
                <FancySelect
                  value=""
                  options={props.envVars.map((item) => ({ value: item.key, label: item.key }))}
                  onChange={(key) => props.setEndpoint(`{{${key}}}`)}
                  placeholder="变量"
                />
              </div>
              <button
                className="endpoint-vars-toggle"
                onClick={() => props.setEnvPanelOpen(!props.envPanelOpen)}
                title="管理环境变量"
                type="button"
              >
                <Variable size={15} />
              </button>
            </div>
          </label>
          <label className="toggle-row">
            <input
              type="checkbox"
              checked={props.useTls}
              onChange={(event) => props.setUseTls(event.target.checked)}
            />
            <span>TLS</span>
          </label>
        </div>

        {props.envPanelOpen && (
          <EnvVarsBody
            envVars={props.envVars}
            setEnvVars={props.setEnvVars}
            endpointTemplate={props.endpoint}
          />
        )}

        <div className="form-strip">
          <label>
            <span>Service</span>
            <FancySelect
              value={props.selectedService}
              options={serviceOptions}
              onChange={props.setSelectedService}
              placeholder="Select service"
            />
          </label>
          <label>
            <span>Method</span>
            <FancySelect
              value={props.selectedMethod}
              options={methodOptions}
              onChange={props.setSelectedMethod}
              placeholder="Select method"
            />
          </label>
          <button className="send-button" onClick={props.sendGrpcRequest} disabled={props.isBusy}>
            <Play size={16} />
            {props.isBusy ? "Sending" : "Send"}
          </button>
        </div>

        <section className="advanced-panel">
          <button
            className="advanced-trigger"
            onClick={() => props.setAdvancedOpen(!props.advancedOpen)}
          >
            <Settings2 size={15} />
            <span>Advanced</span>
            <small>{props.grpcPath || "method details"}</small>
            <ChevronDown size={15} className={props.advancedOpen ? "open" : ""} />
          </button>

          {props.advancedOpen && (
            <div className="advanced-body">
              {props.selectedMethodData && (
                <div className="method-signature">
                  <span>{props.selectedMethodData.requestType}</span>
                  <strong>→</strong>
                  <span>{props.selectedMethodData.responseType}</span>
                  <small>
                    {props.selectedMethodData.clientStreaming || props.selectedMethodData.serverStreaming
                      ? "streaming"
                      : "unary"}
                  </small>
                </div>
              )}

              <div className="advanced-fields">
                <label className="full-method-box">
                  <span>Full method</span>
                  <input
                    value={props.fullMethod || props.grpcPath}
                    onChange={(event) => props.setFullMethod(event.target.value)}
                    placeholder="/package.Service/Method"
                  />
                </label>

                <label className="full-method-box">
                  <span>Authority</span>
                  <input
                    value={props.authority}
                    onChange={(event) => props.setAuthority(event.target.value)}
                    placeholder="package.name"
                  />
                </label>
              </div>


              <section className="request-schema">
                <div className="field-tree">
                  {props.requestFields.length ? (
                    props.requestFields.map((field) => (
                      <FieldNode field={field} key={field.name} depth={0} />
                    ))
                  ) : (
                    <span className="field-empty">Select a method to load request structure.</span>
                  )}
                </div>
              </section>
            </div>
          )}
        </section>

        <div className="metadata-box">
          <span>Metadata</span>
          <textarea
            value={props.metadata}
            onChange={(event) => props.setMetadata(event.target.value)}
            spellCheck={false}
          />
        </div>

        <section className="json-grid">
          <JsonEditor title="Request" value={props.requestJson} onChange={props.setRequestJson} />
          <JsonEditor title="Response" value={props.responseJson} readOnly />
        </section>
      </section>
    </div>
  );
}

function EnvVarsBody({
  envVars,
  setEnvVars,
  endpointTemplate,
}: {
  envVars: EnvVariable[];
  setEnvVars: (vars: EnvVariable[]) => void;
  endpointTemplate: string;
}) {
  const preview = expandTemplate(endpointTemplate, envVars);

  function updateKey(index: number, key: string) {
    setEnvVars(envVars.map((item, i) => (i === index ? { ...item, key } : item)));
  }

  function updateValue(index: number, value: string) {
    setEnvVars(envVars.map((item, i) => (i === index ? { ...item, value } : item)));
  }

  function removeVar(index: number) {
    setEnvVars(envVars.filter((_, i) => i !== index));
  }

  function addVar() {
    setEnvVars([...envVars, { key: "", value: "" }]);
  }

  return (
    <section className="env-panel">
      <div className="env-summary">
        <span>实际地址</span>
        <code>{preview || "—"}</code>
      </div>
      <div className="env-body">
        <p className="env-hint">
          在 Endpoint 中用 <code>{"{{KEY}}"}</code> 引用变量，发送时会自动替换为对应值。未匹配的
          <code>{"{{KEY}}"}</code> 原样保留。
        </p>
        {envVars.length ? (
          envVars.map((variable, index) => (
            <div className="env-row" key={index}>
              <input
                className="env-key"
                value={variable.key}
                onChange={(event) => updateKey(index, event.target.value)}
                placeholder="KEY"
                spellCheck={false}
              />
              <input
                className="env-value"
                value={variable.value}
                onChange={(event) => updateValue(index, event.target.value)}
                placeholder="value"
                spellCheck={false}
              />
              <button className="env-remove" onClick={() => removeVar(index)} title="删除变量" type="button">
                <X size={15} />
              </button>
            </div>
          ))
        ) : (
          <div className="env-empty">还没有变量，点击下方添加。</div>
        )}
        <button className="env-add" onClick={addVar} type="button">
          <Plus size={14} />
          添加变量
        </button>
      </div>
    </section>
  );
}

function FancySelect({
  value,
  options,
  onChange,
  placeholder,
}: {
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
  placeholder: string;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const selected = options.find((option) => option.value === value);

  useEffect(() => {
    if (!open) return;
    const handlePointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", handlePointerDown);
    return () => document.removeEventListener("mousedown", handlePointerDown);
  }, [open]);

  function chooseOption(event: ReactMouseEvent<HTMLButtonElement>, nextValue: string) {
    event.preventDefault();
    event.stopPropagation();
    onChange(nextValue);
    setOpen(false);
  }

  return (
    <div className="fancy-select" ref={rootRef}>
      <button
        className="select-trigger"
        type="button"
        onMouseDown={(event) => {
          event.preventDefault();
          setOpen((current) => !current);
        }}
      >
        <span>{selected?.label || placeholder}</span>
        <ChevronDown size={16} />
      </button>
      {open && (
        <div className="select-menu">
          {options.length ? (
            options.map((option) => (
              <button
                className={`select-option ${option.value === value ? "selected" : ""}`}
                key={option.value}
                type="button"
                onMouseDown={(event) => chooseOption(event, option.value)}
              >
                <span>{option.label}</span>
                {option.value === value && <Check size={15} />}
              </button>
            ))
          ) : (
            <div className="select-empty">No options</div>
          )}
        </div>
      )}
    </div>
  );
}

function FieldNode({ field, depth }: { field: FieldInfo; depth: number }) {
  return (
    <div className="field-node" style={{ paddingLeft: depth * 14 }}>
      <div className="field-row">
        <span className="field-name">{field.jsonName}</span>
        <span className="field-type">{field.fieldType}</span>
        {field.required && <span className="field-badge">required</span>}
      </div>
      {field.children.map((child) => (
        <FieldNode field={child} key={`${field.name}-${child.name}`} depth={depth + 1} />
      ))}
    </div>
  );
}

function JsonEditor({
  title,
  value,
  onChange,
  readOnly,
}: {
  title: string;
  value: string;
  onChange?: (value: string) => void;
  readOnly?: boolean;
}) {
  return (
    <section className="json-pane">
      <div className="panel-header">{title}</div>
      <Editor
        height="100%"
        language="json"
        theme="protohub-light"
        value={value}
        beforeMount={configureProtoEditor}
        onChange={(next) => onChange?.(next ?? "")}
        options={{
          readOnly,
          minimap: { enabled: false },
          fontSize: 13,
          lineHeight: 21,
          scrollBeyondLastLine: false,
          automaticLayout: true,
        }}
      />
    </section>
  );
}

const protobufKeywords = [
  "syntax",
  "package",
  "import",
  "option",
  "service",
  "rpc",
  "returns",
  "message",
  "enum",
  "oneof",
  "map",
  "reserved",
  "repeated",
  "optional",
  "required",
  "stream",
  "extend",
  "extensions",
  "public",
  "weak",
  "true",
  "false",
];

const protobufTypes = [
  "double",
  "float",
  "int32",
  "int64",
  "uint32",
  "uint64",
  "sint32",
  "sint64",
  "fixed32",
  "fixed64",
  "sfixed32",
  "sfixed64",
  "bool",
  "string",
  "bytes",
];

const configureProtoEditor: BeforeMount = (monaco) => {
  if (!monaco.languages.getLanguages().some((language) => language.id === "protobuf")) {
    monaco.languages.register({ id: "protobuf" });
  }

  monaco.languages.setMonarchTokensProvider("protobuf", {
    defaultToken: "",
    tokenPostfix: ".proto",
    keywords: protobufKeywords,
    typeKeywords: protobufTypes,
    tokenizer: {
      root: [
        [/\/\/.*$/, "comment"],
        [/\/\*/, "comment", "@comment"],
        [/"([^"\\]|\\.)*$/, "string.invalid"],
        [/"/, "string", "@string"],
        [/[{}()[\];,.<>:=]/, "delimiter"],
        [/\d+\.\d+([eE][\-+]?\d+)?/, "number.float"],
        [/\d+/, "number"],
        [
          /[a-zA-Z_][\w.]*/,
          {
            cases: {
              "@keywords": "keyword",
              "@typeKeywords": "type",
              "@default": "identifier",
            },
          },
        ],
      ],
      comment: [
        [/[^/*]+/, "comment"],
        [/\*\//, "comment", "@pop"],
        [/[/*]/, "comment"],
      ],
      string: [
        [/[^\\"]+/, "string"],
        [/\\./, "string.escape"],
        [/"/, "string", "@pop"],
      ],
    },
  });

  monaco.editor.defineTheme("protohub-light", {
    base: "vs",
    inherit: true,
    rules: [
      { token: "keyword", foreground: "0b6f63", fontStyle: "bold" },
      { token: "type", foreground: "7c3aed" },
      { token: "identifier", foreground: "17202a" },
      { token: "comment", foreground: "7d8a99", fontStyle: "italic" },
      { token: "string", foreground: "b45309" },
      { token: "number", foreground: "2563eb" },
      { token: "delimiter", foreground: "64748b" },
    ],
    colors: {
      "editor.background": "#ffffff",
      "editor.foreground": "#17202a",
      "editorLineNumber.foreground": "#94a3b8",
      "editorLineNumber.activeForeground": "#0f766e",
      "editor.selectionBackground": "#bfdbfe",
      "editor.inactiveSelectionBackground": "#dbeafe",
      "editor.lineHighlightBackground": "#f8fafc",
      "editorCursor.foreground": "#0f766e",
      "editorIndentGuide.background1": "#e2e8f0",
      "editorIndentGuide.activeBackground1": "#94a3b8",
    },
  });
};

function TreeNode({
  node,
  activeFile,
  onOpen,
  depth,
}: {
  node: FileNode;
  activeFile: string;
  onOpen: (path: string) => void;
  depth: number;
}) {
  const [expanded, setExpanded] = useState(depth < 2);
  const isActive = activeFile === node.path;
  const paddingLeft = 10 + depth * 14;

  if (node.isDir) {
    return (
      <div>
        <button className="tree-row directory" style={{ paddingLeft }} onClick={() => setExpanded(!expanded)}>
          {expanded ? <ChevronDown size={15} /> : <ChevronRight size={15} />}
          <span>{node.name}</span>
        </button>
        {expanded &&
          node.children.map((child) => (
            <TreeNode key={child.path} node={child} activeFile={activeFile} onOpen={onOpen} depth={depth + 1} />
          ))}
      </div>
    );
  }

  return (
    <button
      className={`tree-row file ${isActive ? "active" : ""}`}
      style={{ paddingLeft }}
      onClick={() => onOpen(node.path)}
    >
      <FileCode2 size={15} />
      <span>{node.name}</span>
    </button>
  );
}

function findFirstProto(node: FileNode): FileNode | null {
  if (!node.isDir && node.name.endsWith(".proto")) return node;
  for (const child of node.children) {
    const result = findFirstProto(child);
    if (result) return result;
  }
  return null;
}

function parseProtoClientSide(content: string, filePath: string): ProtoAnalysis | undefined {
  const packageMatch = content.match(/^\s*package\s+([\w.]+)\s*;/m);
  const packageName = packageMatch ? packageMatch[1] : "";
  const services: ProtoService[] = [];
  const symbols: ProtoSymbol[] = [];
  const diagnostics: string[] = [];

  const lines = content.split(/\r?\n/);
  let currentService: ProtoService | undefined;

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index];
    const lineNumber = index + 1;

    const serviceMatch = line.match(/^\s*service\s+(\w+)\s*\{/);
    if (serviceMatch) {
      currentService = {
        name: serviceMatch[1],
        fullName: packageName ? `${packageName}.${serviceMatch[1]}` : serviceMatch[1],
        line: lineNumber,
        methods: [],
      };
      services.push(currentService);
      continue;
    }

    if (currentService) {
      const rpcMatch = line.match(
        /^\s*rpc\s+(\w+)\s*\(\s*(\w+)\s*\)\s*returns\s*\(\s*(\w+)\s*\)\s*;?/,
      );
      if (rpcMatch) {
        currentService.methods.push({
          name: rpcMatch[1],
          requestType: rpcMatch[2],
          responseType: rpcMatch[3],
          clientStreaming: false,
          serverStreaming: false,
          line: lineNumber,
        });
        continue;
      }
      if (line.trim().startsWith("}")) {
        currentService = undefined;
        continue;
      }
    }

    const messageMatch = line.match(/^\s*message\s+(\w+)\s*[\{;/]/);
    if (messageMatch) {
      symbols.push({ name: messageMatch[1], kind: "message", line: lineNumber });
      continue;
    }

    const enumMatch = line.match(/^\s*enum\s+(\w+)\s*[\{;/]/);
    if (enumMatch) {
      symbols.push({ name: enumMatch[1], kind: "enum", line: lineNumber });
    }
  }

  return services.length || symbols.length || packageName
    ? {
        packageName,
        services,
        symbols,
        descriptorAvailable: false,
        diagnostics: diagnostics.length ? diagnostics : ["Client-side preview only"],
      }
    : undefined;
}

function findDefinitionInContent(content: string, word: string) {
  const parts = word.split(".").filter(Boolean);
  const cleanWord = parts.length ? parts[parts.length - 1] : word;
  const escaped = escapeRegExp(cleanWord);
  const definitionPattern = new RegExp(
    `^\\s*(message|enum|service|rpc)\\s+${escaped}(\\s|\\(|\\{)`,
  );

  const lines = content.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    if (definitionPattern.test(lines[index])) return index + 1;
  }
  return undefined;
}

function parseProtoImports(content: string) {
  const imports: string[] = [];
  const importPattern = /^\s*import\s+(?:public\s+|weak\s+)?["']([^"']+)["']\s*;/;
  for (const line of content.split(/\r?\n/)) {
    const match = line.match(importPattern);
    if (match) imports.push(match[1]);
  }
  return imports;
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}


function fileName(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

function gitStatusLabel(status: string) {
  if (status === "??") return "U";
  if (status.includes("R")) return "R";
  if (status.includes("D")) return "D";
  if (status.includes("A")) return "A";
  if (status.includes("M")) return "M";
  return status.trim() || "?";
}

function parseMetadata(value: string) {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [key, ...rest] = line.split(":");
      return { key: key.trim(), value: rest.join(":").trim() };
    })
    .filter((item) => item.key && item.value);
}

use anyhow::{anyhow, Context};
use bytes::Buf;
use prost::Message;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor,
    SerializeOptions, ServiceDescriptor,
};
use prost_types::FileDescriptorSet;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};
use tauri::Manager;
use tonic::{
    client::Grpc,
    codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder},
    metadata::{Ascii, MetadataKey, MetadataValue},
    transport::Endpoint,
    Code, Request,
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FileNode {
    name: String,
    path: String,
    is_dir: bool,
    children: Vec<FileNode>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProtoSymbol {
    name: String,
    kind: String,
    line: usize,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProtoMethodInfo {
    name: String,
    request_type: String,
    response_type: String,
    client_streaming: bool,
    server_streaming: bool,
    line: usize,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ProtoServiceInfo {
    name: String,
    full_name: String,
    line: usize,
    methods: Vec<ProtoMethodInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProtoAnalysis {
    package_name: String,
    services: Vec<ProtoServiceInfo>,
    symbols: Vec<ProtoSymbol>,
    descriptor_available: bool,
    diagnostics: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeRequest {
    file_path: String,
    content: String,
    include_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataInput {
    key: String,
    value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GrpcRequestInput {
    endpoint: String,
    file_path: String,
    include_paths: Vec<String>,
    service: String,
    method: String,
    grpc_path: Option<String>,
    authority: Option<String>,
    request_json: Value,
    metadata: Vec<MetadataInput>,
    use_tls: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GrpcResponseOutput {
    status: String,
    response_json: Value,
    elapsed_ms: u128,
    grpc_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MethodTemplateInput {
    file_path: String,
    include_paths: Vec<String>,
    service: String,
    method: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MethodTemplateOutput {
    request_type: String,
    response_type: String,
    request_json: Value,
    response_json: Value,
    request_fields: Vec<FieldInfo>,
    response_fields: Vec<FieldInfo>,
    service_options: Value,
    method_options: Value,
    request_options: Value,
    response_options: Value,
    rpc_source: String,
    enum_source: String,
    grpc_path: String,
    client_streaming: bool,
    server_streaming: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FieldInfo {
    name: String,
    json_name: String,
    field_type: String,
    number: u32,
    cardinality: String,
    oneof: Option<String>,
    enum_values: Vec<String>,
    options: Value,
    repeated: bool,
    map: bool,
    required: bool,
    children: Vec<FieldInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitWorkspaceInfo {
    repository_root: String,
    remote_url: String,
    current_branch: String,
    available_branches: Vec<String>,
    default_branch: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitPullOutput {
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitChange {
    path: String,
    relative_path: String,
    original_relative_path: String,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitFileDiff {
    original: String,
    modified: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GitCommitOutput {
    commit: String,
    branch: String,
    message: String,
}

#[tauri::command]
fn read_workspace(root: String) -> Result<FileNode, String> {
    build_file_tree(Path::new(&root)).map_err(to_string)
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    fs::read_to_string(path).map_err(to_string)
}

#[tauri::command]
fn write_text_file(path: String, content: String) -> Result<(), String> {
    fs::write(path, content).map_err(to_string)
}

#[tauri::command]
fn get_git_workspace_info(root: String) -> Result<Option<GitWorkspaceInfo>, String> {
    let root_path = Path::new(&root);
    let repository_root = match git_output(root_path, &["rev-parse", "--show-toplevel"]) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    let repository_path = PathBuf::from(&repository_root);
    let remote_url = match git_output(&repository_path, &["remote", "get-url", "origin"]) {
        Ok(value) if !value.is_empty() => value,
        _ => return Ok(None),
    };
    let current_branch =
        git_output(&repository_path, &["branch", "--show-current"]).unwrap_or_default();
    let mut available_branches = Vec::new();
    for branch in ["main", "master"] {
        if git_command_succeeds(
            &repository_path,
            &[
                "show-ref",
                "--verify",
                "--quiet",
                &format!("refs/remotes/origin/{branch}"),
            ],
        ) {
            available_branches.push(branch.to_string());
        }
    }
    if available_branches.is_empty() {
        return Ok(None);
    }
    let default_branch = if available_branches
        .iter()
        .any(|item| item == &current_branch)
    {
        current_branch.clone()
    } else {
        available_branches[0].clone()
    };

    Ok(Some(GitWorkspaceInfo {
        repository_root,
        remote_url,
        current_branch,
        available_branches,
        default_branch,
    }))
}

#[tauri::command]
fn pull_git_branch(root: String, branch: String) -> Result<GitPullOutput, String> {
    if branch != "main" && branch != "master" {
        return Err("Only main or master can be auto-pulled".to_string());
    }
    let repository_root = git_output(Path::new(&root), &["rev-parse", "--show-toplevel"])?;
    let repository_path = PathBuf::from(repository_root);
    git_output(&repository_path, &["remote", "get-url", "origin"])?;
    let current_branch =
        git_output(&repository_path, &["branch", "--show-current"]).unwrap_or_default();
    let output = if current_branch == branch {
        git_output(&repository_path, &["pull", "--ff-only", "origin", &branch])?
    } else {
        let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
        git_output(&repository_path, &["fetch", "origin", &refspec])?
    };
    Ok(GitPullOutput {
        message: if output.is_empty() {
            format!("{branch} is up to date")
        } else {
            output
        },
    })
}

#[tauri::command]
fn get_git_changes(root: String) -> Result<Vec<GitChange>, String> {
    let repository_root = git_output(Path::new(&root), &["rev-parse", "--show-toplevel"])?;
    let repository_path = PathBuf::from(repository_root);
    let output = git_output(
        &repository_path,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    Ok(parse_git_changes(&output, &repository_path))
}

fn parse_git_changes(output: &str, repository_path: &Path) -> Vec<GitChange> {
    output
        .lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let status = line[..2].trim().to_string();
            let paths = line[3..].rsplit_once(" -> ");
            let display_path = paths
                .map(|(_, destination)| destination)
                .unwrap_or(&line[3..])
                .trim_matches('"')
                .to_string();
            let original_relative_path = paths
                .map(|(source, _)| source)
                .unwrap_or(&display_path)
                .trim_matches('"')
                .to_string();
            Some(GitChange {
                path: repository_path
                    .join(&display_path)
                    .to_string_lossy()
                    .to_string(),
                relative_path: display_path,
                original_relative_path,
                status,
            })
        })
        .collect()
}

#[tauri::command]
fn get_git_file_diff(
    root: String,
    relative_path: String,
    original_relative_path: String,
    status: String,
) -> Result<GitFileDiff, String> {
    for candidate in [&relative_path, &original_relative_path] {
        let path = Path::new(candidate);
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("Invalid repository path".to_string());
        }
    }
    let relative = Path::new(&relative_path);
    let repository_root = git_output(Path::new(&root), &["rev-parse", "--show-toplevel"])?;
    let repository_path = PathBuf::from(repository_root);
    let head_path = format!("HEAD:{original_relative_path}");
    let original = if status == "??" || status.contains('A') {
        String::new()
    } else {
        match git_output(&repository_path, &["show", "--no-textconv", &head_path]) {
            Ok(content) => content,
            Err(head_error) => {
                let index_path = format!(":{original_relative_path}");
                git_output(&repository_path, &["show", "--no-textconv", &index_path]).map_err(
                    |index_error| {
                        format!(
                            "Unable to read the Git baseline for {original_relative_path}. HEAD: {head_error}; index: {index_error}"
                        )
                    },
                )?
            }
        }
    };
    let modified = if status.contains('D') {
        String::new()
    } else {
        let working_path = repository_path.join(relative);
        fs::read_to_string(&working_path).map_err(|error| {
            format!(
                "Unable to read working file {}: {error}",
                working_path.display()
            )
        })?
    };
    Ok(GitFileDiff { original, modified })
}

#[tauri::command]
fn commit_and_push_git(root: String, message: String) -> Result<GitCommitOutput, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Commit message cannot be empty".to_string());
    }
    let repository_root = git_output(Path::new(&root), &["rev-parse", "--show-toplevel"])?;
    let repository_path = PathBuf::from(repository_root);
    let branch = git_output(&repository_path, &["branch", "--show-current"])?;
    if branch.is_empty() {
        return Err("Cannot commit while HEAD is detached".to_string());
    }
    git_output(&repository_path, &["remote", "get-url", "origin"])?;
    git_output(&repository_path, &["add", "-A"])?;
    if !git_command_succeeds(&repository_path, &["diff", "--cached", "--quiet"]) {
        git_output(&repository_path, &["commit", "-m", message])?;
    } else {
        return Err("There are no changes to commit".to_string());
    }
    let commit = git_output(&repository_path, &["rev-parse", "--short", "HEAD"])?;
    let push_output =
        git_output(&repository_path, &["push", "origin", &branch]).map_err(|error| {
            format!("Commit {commit} was created locally, but push failed: {error}")
        })?;
    Ok(GitCommitOutput {
        commit,
        branch,
        message: if push_output.is_empty() {
            "Push completed".to_string()
        } else {
            push_output
        },
    })
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("Unable to run git: {error}"))?;
    if output.status.success() {
        // Leading whitespace is significant for porcelain status output and
        // source file contents. Only remove command-ending line breaks.
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_string())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if message.is_empty() {
            "Git command failed".to_string()
        } else {
            message
        })
    }
}

fn git_command_succeeds(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[tauri::command]
fn analyze_proto(input: AnalyzeRequest) -> Result<ProtoAnalysis, String> {
    let mut analysis = parse_proto_lightweight(&input.file_path, &input.content);
    match compile_descriptor(&input.file_path, &input.include_paths) {
        Ok(pool) => {
            analysis.descriptor_available = true;
            analysis.services = merge_service_lines(
                analysis.services.clone(),
                services_from_pool(&pool, &input.file_path),
            );
        }
        Err(error) => {
            analysis
                .diagnostics
                .push(format!("Descriptor unavailable: {error}"));
        }
    }
    Ok(analysis)
}

#[tauri::command]
async fn invoke_grpc(input: GrpcRequestInput) -> Result<GrpcResponseOutput, String> {
    invoke_grpc_inner(input).await.map_err(to_string)
}

#[tauri::command]
fn get_method_template(input: MethodTemplateInput) -> Result<MethodTemplateOutput, String> {
    get_method_template_inner(input).map_err(to_string)
}

fn get_method_template_inner(input: MethodTemplateInput) -> anyhow::Result<MethodTemplateOutput> {
    let pool = compile_descriptor(&input.file_path, &input.include_paths)?;
    let service = pool
        .get_service_by_name(&input.service)
        .or_else(|| find_service_by_suffix(&pool, &input.service))
        .ok_or_else(|| anyhow!("service not found: {}", input.service))?;
    let method = service
        .methods()
        .find(|method| method.name() == input.method)
        .ok_or_else(|| anyhow!("method not found: {}", input.method))?;
    let request_descriptor = method.input();
    let response_descriptor = method.output();
    let grpc_path = format!("/{}/{}", service.full_name(), method.name());
    let source = fs::read_to_string(&input.file_path).unwrap_or_default();

    Ok(MethodTemplateOutput {
        request_type: request_descriptor.full_name().to_string(),
        response_type: response_descriptor.full_name().to_string(),
        request_json: message_template_json(&request_descriptor, 0),
        response_json: message_template_json(&response_descriptor, 0),
        request_fields: message_fields(&request_descriptor, 0),
        response_fields: message_fields(&response_descriptor, 0),
        service_options: serialize_message_json(&service.options()).unwrap_or(Value::Null),
        method_options: serialize_message_json(&method.options()).unwrap_or(Value::Null),
        request_options: serialize_message_json(&request_descriptor.options())
            .unwrap_or(Value::Null),
        response_options: serialize_message_json(&response_descriptor.options())
            .unwrap_or(Value::Null),
        rpc_source: extract_rpc_source(&source, method.name()),
        enum_source: collect_imported_enums(&input.include_paths, &input.file_path),
        grpc_path,
        client_streaming: method.is_client_streaming(),
        server_streaming: method.is_server_streaming(),
    })
}

fn extract_rpc_source(source: &str, method_name: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let needle = format!("rpc {method_name}");
    let Some(start) = lines.iter().position(|line| line.contains(&needle)) else {
        return String::new();
    };
    extract_block_from_lines(&lines, start)
}

fn extract_named_blocks(source: &str, keyword: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_start().starts_with(&format!("{keyword} ")))
        .map(|(index, _)| extract_block_from_lines(&lines, index))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn collect_imported_enums(include_paths: &[String], current_file: &str) -> String {
    let current_path = Path::new(current_file);
    let fallback_root = current_path.parent().unwrap_or_else(|| Path::new("."));
    let workspace_root = include_paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
        .min_by_key(|path| path.components().count())
        .unwrap_or_else(|| fallback_root.to_path_buf());
    let workspace_root = fs::canonicalize(&workspace_root).unwrap_or(workspace_root);
    let mut queue = VecDeque::from([current_path.to_path_buf()]);
    let mut visited = HashSet::new();
    let mut groups = Vec::new();
    while let Some(path) = queue.pop_front() {
        let canonical = fs::canonicalize(&path).unwrap_or(path);
        if !visited.insert(canonical.clone()) {
            continue;
        }
        let Ok(source) = fs::read_to_string(&canonical) else {
            continue;
        };
        let enums = extract_named_blocks(&source, "enum");
        if !enums.is_empty() {
            let display_path = canonical
                .strip_prefix(&workspace_root)
                .unwrap_or(&canonical);
            groups.push(format!("// Source: {}\n{enums}", display_path.display()));
        }
        let source_dir = canonical.parent().unwrap_or(fallback_root);
        for import_path in parse_proto_imports(&source) {
            let candidates = std::iter::once(source_dir.join(&import_path)).chain(
                include_paths
                    .iter()
                    .map(|include_path| Path::new(include_path).join(&import_path)),
            );
            if let Some(resolved) = candidates.into_iter().find(|candidate| candidate.is_file()) {
                queue.push_back(resolved);
            }
        }
    }
    groups.join("\n\n")
}

fn parse_proto_imports(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("import ") {
                return None;
            }
            let start = line.find('"')? + 1;
            let end = line[start..].find('"')? + start;
            Some(line[start..end].to_string())
        })
        .collect()
}

fn extract_block_from_lines(lines: &[&str], start: usize) -> String {
    let mut first = start;
    while first > 0 && lines[first - 1].trim_start().starts_with("//") {
        first -= 1;
    }
    let mut depth = 0_i32;
    let mut opened = false;
    let mut end = start;
    for (index, line) in lines.iter().enumerate().skip(start) {
        for character in line.chars() {
            if character == '{' {
                depth += 1;
                opened = true;
            } else if character == '}' {
                depth -= 1;
            }
        }
        end = index;
        if (opened && depth <= 0) || (!opened && line.contains(';')) {
            break;
        }
    }
    lines[first..=end].join("\n")
}

async fn invoke_grpc_inner(input: GrpcRequestInput) -> anyhow::Result<GrpcResponseOutput> {
    let started = std::time::Instant::now();
    let pool = compile_descriptor(&input.file_path, &input.include_paths)?;
    let service = pool
        .get_service_by_name(&input.service)
        .or_else(|| find_service_by_suffix(&pool, &input.service))
        .ok_or_else(|| anyhow!("service not found: {}", input.service))?;
    let method = service
        .methods()
        .find(|method| method.name() == input.method)
        .ok_or_else(|| anyhow!("method not found: {}", input.method))?;

    if method.is_client_streaming() || method.is_server_streaming() {
        return Err(anyhow!(
            "streaming RPC is recognized but not supported in this MVP"
        ));
    }

    let request_descriptor = method.input();
    let response_descriptor = method.output();
    let request_json = normalize_request_json(&request_descriptor, input.request_json);
    let request_message = DynamicMessage::deserialize(request_descriptor, request_json)
        .context("request JSON does not match the selected protobuf message")?;

    let authority = input
        .authority
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| service.package_name())
        .to_string();

    let scheme = if input.use_tls { "https" } else { "http" };
    let uri = if input.endpoint.starts_with("http://") || input.endpoint.starts_with("https://") {
        input.endpoint.clone()
    } else {
        format!("{scheme}://{}", input.endpoint)
    };

    let mut endpoint = Endpoint::from_shared(uri)?;
    if !authority.is_empty() {
        let origin = format!("{scheme}://{authority}").parse()?;
        endpoint = endpoint.origin(origin);
    }
    let channel = endpoint.connect().await?;
    let mut grpc = Grpc::new(channel);
    grpc.ready().await.map_err(|error| anyhow!("{error}"))?;

    let path_text = input
        .grpc_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(normalize_grpc_path)
        .unwrap_or_else(|| format!("/{}/{}", service.full_name(), method.name()));
    let path = http::uri::PathAndQuery::from_str(&path_text)?;
    let codec = DynamicCodec::new(response_descriptor);
    let mut request = Request::new(request_message);
    for item in input.metadata {
        if item.key.trim().is_empty() {
            continue;
        }
        let key = MetadataKey::<Ascii>::from_str(item.key.trim())?;
        let value = MetadataValue::from_str(item.value.trim())?;
        request.metadata_mut().insert(key, value);
    }

    let response = match grpc.unary(request, path.clone(), codec).await {
        Ok(response) => response,
        Err(status) => {
            return Err(anyhow!(
                "{}",
                grpc_error_message(
                    status.code(),
                    status.message(),
                    &path.to_string(),
                    &input.endpoint,
                    &authority,
                    input.use_tls
                )
            ));
        }
    };
    let message = response.into_inner();
    let response_json = serialize_message_json(&message)?;

    Ok(GrpcResponseOutput {
        status: "OK".to_string(),
        response_json,
        elapsed_ms: started.elapsed().as_millis(),
        grpc_path: path.to_string(),
    })
}

fn normalize_grpc_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn serialize_message_json(message: &DynamicMessage) -> anyhow::Result<Value> {
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::new(&mut bytes);
    message.serialize_with_options(
        &mut serializer,
        &SerializeOptions::new().use_proto_field_name(true),
    )?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn normalize_request_json(message: &MessageDescriptor, value: Value) -> Value {
    let Value::Object(mut source) = value else {
        return value;
    };
    let mut target = serde_json::Map::new();

    for field in message.fields() {
        let candidate_keys = [
            field.json_name().to_string(),
            field.name().to_string(),
            field.name().replace('_', ""),
            field.json_name().to_ascii_lowercase(),
        ];
        let mut field_value = None;
        for key in candidate_keys {
            if let Some(value) = source.remove(&key) {
                field_value = Some(value);
                break;
            }
        }

        if let Some(value) = field_value {
            target.insert(
                field.json_name().to_string(),
                normalize_field_json(&field, value),
            );
        }
    }

    for (key, value) in source {
        target.insert(key, value);
    }

    Value::Object(target)
}

fn normalize_field_json(field: &FieldDescriptor, value: Value) -> Value {
    match field.kind() {
        Kind::Message(message) if field.is_list() => {
            if let Value::Array(items) = value {
                Value::Array(
                    items
                        .into_iter()
                        .map(|item| normalize_request_json(&message, item))
                        .collect(),
                )
            } else {
                value
            }
        }
        Kind::Message(message) if !field.is_map() => normalize_request_json(&message, value),
        _ => value,
    }
}

fn grpc_error_message(
    code: Code,
    message: &str,
    path: &str,
    endpoint: &str,
    authority: &str,
    use_tls: bool,
) -> String {
    let hint = match code {
        Code::Unimplemented => {
            "The server is reachable, but this gRPC method path is not registered. Check whether the endpoint is native gRPC, grpc-web/TRPC/HTTP gateway, whether TLS is required, and whether the service package/name matches the server."
        }
        Code::Unavailable => {
            "The endpoint is unreachable or refused the connection. Check host, port, network, and TLS."
        }
        Code::Internal => {
            "The server accepted the call but failed while handling it. Check server logs and response type."
        }
        _ => "The server returned a gRPC status error.",
    };

    format!(
        "gRPC {code}: {message}\npath: {path}\nendpoint: {endpoint}\nauthority: {authority}\ntls: {use_tls}\nhint: {hint}"
    )
}

fn message_template_json(message: &MessageDescriptor, depth: usize) -> Value {
    let mut object = serde_json::Map::new();
    for field in message.fields() {
        object.insert(
            field.name().to_string(),
            field_template_json(&field, depth + 1),
        );
    }
    Value::Object(object)
}

fn field_template_json(field: &FieldDescriptor, depth: usize) -> Value {
    if field.is_map() {
        return Value::Object(serde_json::Map::new());
    }
    if field.is_list() {
        return Value::Array(vec![single_field_template_json(field, depth)]);
    }
    single_field_template_json(field, depth)
}

fn single_field_template_json(field: &FieldDescriptor, depth: usize) -> Value {
    match field.kind() {
        Kind::Double | Kind::Float => Value::from(0.0),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 | Kind::Uint32 | Kind::Fixed32 => {
            Value::from(0)
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 | Kind::Uint64 | Kind::Fixed64 => {
            Value::from("0")
        }
        Kind::Bool => Value::from(false),
        Kind::String => Value::from(""),
        Kind::Bytes => Value::from(""),
        Kind::Enum(enum_descriptor) => enum_descriptor
            .values()
            .next()
            .map(|value| Value::from(value.name()))
            .unwrap_or(Value::Null),
        Kind::Message(message) => {
            if depth > 4 {
                Value::Object(serde_json::Map::new())
            } else {
                message_template_json(&message, depth)
            }
        }
    }
}

fn message_fields(message: &MessageDescriptor, depth: usize) -> Vec<FieldInfo> {
    message
        .fields()
        .map(|field| field_info(&field, depth + 1))
        .collect()
}

fn field_info(field: &FieldDescriptor, depth: usize) -> FieldInfo {
    let children = match field.kind() {
        Kind::Message(message) if !field.is_map() && depth <= 4 => message_fields(&message, depth),
        Kind::Message(message) if field.is_map() && depth <= 4 => {
            vec![field_info(&message.map_entry_value_field(), depth + 1)]
        }
        _ => Vec::new(),
    };

    FieldInfo {
        name: field.name().to_string(),
        json_name: field.json_name().to_string(),
        field_type: field_type_label(field),
        number: field.number(),
        cardinality: format!("{:?}", field.cardinality()).to_lowercase(),
        oneof: field
            .containing_oneof()
            .map(|oneof| oneof.name().to_string()),
        enum_values: match field.kind() {
            Kind::Enum(descriptor) => descriptor
                .values()
                .map(|value| format!("{} = {}", value.name(), value.number()))
                .collect(),
            _ => Vec::new(),
        },
        options: serialize_message_json(&field.options()).unwrap_or(Value::Null),
        repeated: field.is_list(),
        map: field.is_map(),
        required: matches!(field.cardinality(), prost_reflect::Cardinality::Required),
        children,
    }
}

fn field_type_label(field: &FieldDescriptor) -> String {
    let base = match field.kind() {
        Kind::Double => "double".to_string(),
        Kind::Float => "float".to_string(),
        Kind::Int32 => "int32".to_string(),
        Kind::Int64 => "int64".to_string(),
        Kind::Uint32 => "uint32".to_string(),
        Kind::Uint64 => "uint64".to_string(),
        Kind::Sint32 => "sint32".to_string(),
        Kind::Sint64 => "sint64".to_string(),
        Kind::Fixed32 => "fixed32".to_string(),
        Kind::Fixed64 => "fixed64".to_string(),
        Kind::Sfixed32 => "sfixed32".to_string(),
        Kind::Sfixed64 => "sfixed64".to_string(),
        Kind::Bool => "bool".to_string(),
        Kind::String => "string".to_string(),
        Kind::Bytes => "bytes".to_string(),
        Kind::Enum(enum_descriptor) => format!("enum {}", enum_descriptor.full_name()),
        Kind::Message(message) => message.full_name().to_string(),
    };

    if field.is_map() {
        format!("map<{}>", base)
    } else if field.is_list() {
        format!("repeated {}", base)
    } else {
        base
    }
}

fn build_file_tree(root: &Path) -> anyhow::Result<FileNode> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("workspace")
        .to_string();
    let mut node = FileNode {
        name,
        path: root.to_string_lossy().to_string(),
        is_dir: root.is_dir(),
        children: Vec::new(),
    };

    if root.is_dir() {
        let mut entries = fs::read_dir(root)?
            .filter_map(Result::ok)
            .filter(|entry| !is_hidden(entry.path().as_path()))
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            let path = entry.path();
            (!path.is_dir(), entry.file_name())
        });
        for entry in entries {
            let path = entry.path();
            if path.is_dir() || path.extension().and_then(|ext| ext.to_str()) == Some("proto") {
                node.children.push(build_file_tree(&path)?);
            }
        }
    }

    Ok(node)
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

fn parse_proto_lightweight(file_path: &str, content: &str) -> ProtoAnalysis {
    let mut package_name = String::new();
    let mut services = Vec::new();
    let mut symbols = Vec::new();
    let mut current_service: Option<ProtoServiceInfo> = None;
    let mut service_depth = 0;

    for (index, line) in content.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("package ") {
            package_name = rest.trim_end_matches(';').trim().to_string();
        }
        if let Some(name) = declaration_name(trimmed, "message ") {
            symbols.push(ProtoSymbol {
                name,
                kind: "message".to_string(),
                line: line_no,
            });
        }
        if let Some(name) = declaration_name(trimmed, "enum ") {
            symbols.push(ProtoSymbol {
                name,
                kind: "enum".to_string(),
                line: line_no,
            });
        }
        if let Some(name) = declaration_name(trimmed, "service ") {
            if let Some(service) = current_service.take() {
                services.push(service);
            }
            service_depth = brace_delta(trimmed).max(1);
            let full_name = if package_name.is_empty() {
                name.clone()
            } else {
                format!("{package_name}.{name}")
            };
            symbols.push(ProtoSymbol {
                name: name.clone(),
                kind: "service".to_string(),
                line: line_no,
            });
            current_service = Some(ProtoServiceInfo {
                name,
                full_name,
                line: line_no,
                methods: Vec::new(),
            });
            continue;
        }
        if current_service.is_some() && trimmed.starts_with("rpc ") {
            if let Some(method) = parse_rpc_line(trimmed, line_no) {
                if let Some(service) = &mut current_service {
                    service.methods.push(method);
                }
            }
        }
        if current_service.is_some() {
            service_depth += brace_delta(trimmed);
            if service_depth <= 0 {
                if let Some(service) = current_service.take() {
                    services.push(service);
                }
                service_depth = 0;
            }
        }
        if current_service.is_none() && trimmed.starts_with('}') {
            if let Some(service) = current_service.take() {
                services.push(service);
            }
        }
    }
    if let Some(service) = current_service {
        services.push(service);
    }

    ProtoAnalysis {
        package_name,
        services,
        symbols,
        descriptor_available: false,
        diagnostics: vec![format!("Analyzed {}", file_path)],
    }
}

fn brace_delta(line: &str) -> i32 {
    line.chars().fold(0, |depth, ch| match ch {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

fn declaration_name(line: &str, prefix: &str) -> Option<String> {
    line.strip_prefix(prefix).map(|rest| {
        rest.split(|ch: char| ch.is_whitespace() || ch == '{')
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

fn parse_rpc_line(line: &str, line_no: usize) -> Option<ProtoMethodInfo> {
    let rest = line.strip_prefix("rpc ")?.trim();
    let name = rest.split('(').next()?.trim().to_string();
    let after_name = rest.split_once('(')?.1;
    let (request_raw, after_request) = after_name.split_once(')')?;
    let after_returns = after_request.split_once("returns")?.1;
    let response_raw = after_returns.split_once('(')?.1.split_once(')')?.0;

    let (client_streaming, request_type) = parse_rpc_type(request_raw);
    let (server_streaming, response_type) = parse_rpc_type(response_raw);

    Some(ProtoMethodInfo {
        name,
        request_type,
        response_type,
        client_streaming,
        server_streaming,
        line: line_no,
    })
}

fn parse_rpc_type(raw: &str) -> (bool, String) {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("stream ") {
        (true, rest.trim().to_string())
    } else {
        (false, trimmed.to_string())
    }
}

fn compile_descriptor(file_path: &str, include_paths: &[String]) -> anyhow::Result<DescriptorPool> {
    let mut includes = include_paths
        .iter()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .collect::<Vec<_>>();
    if let Some(parent) = Path::new(file_path).parent() {
        includes.push(parent.to_path_buf());
    }
    let includes = dedupe_paths(includes);
    let descriptor_set = protox::compile([PathBuf::from(file_path)], includes)
        .with_context(|| format!("failed to compile protobuf descriptor for {file_path}"))?;
    let bytes = protox::prost::Message::encode_to_vec(&descriptor_set);
    let descriptor_set = FileDescriptorSet::decode(bytes.as_slice())?;
    DescriptorPool::from_file_descriptor_set(descriptor_set).map_err(Into::into)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut result = Vec::new();
    for path in paths {
        if !result.iter().any(|item| item == &path) {
            result.push(path);
        }
    }
    result
}

fn services_from_pool(pool: &DescriptorPool, file_path: &str) -> Vec<ProtoServiceInfo> {
    let mut services = Vec::new();
    for service in pool.services() {
        if descriptor_file_matches(service.parent_file().name(), file_path) {
            services.push(service_from_descriptor(&service));
        }
    }
    services
}

fn descriptor_file_matches(descriptor_name: &str, file_path: &str) -> bool {
    let normalized_descriptor = descriptor_name.replace('\\', "/");
    let normalized_file = file_path.replace('\\', "/");
    normalized_descriptor == normalized_file
        || normalized_file.ends_with(&normalized_descriptor)
        || Path::new(&normalized_descriptor).file_name() == Path::new(&normalized_file).file_name()
}

fn merge_service_lines(
    lightweight: Vec<ProtoServiceInfo>,
    descriptor_services: Vec<ProtoServiceInfo>,
) -> Vec<ProtoServiceInfo> {
    descriptor_services
        .into_iter()
        .map(|mut service| {
            if let Some(parsed_service) = lightweight.iter().find(|item| {
                item.name == service.name
                    || item.full_name == service.full_name
                    || item.full_name.ends_with(&service.name)
            }) {
                service.line = parsed_service.line;
                for method in &mut service.methods {
                    if let Some(parsed_method) = parsed_service
                        .methods
                        .iter()
                        .find(|item| item.name == method.name)
                    {
                        method.line = parsed_method.line;
                    }
                }
            }
            service
        })
        .collect()
}

fn service_from_descriptor(service: &ServiceDescriptor) -> ProtoServiceInfo {
    ProtoServiceInfo {
        name: service.name().to_string(),
        full_name: service.full_name().to_string(),
        line: 1,
        methods: service
            .methods()
            .map(|method| method_from_descriptor(&method))
            .collect(),
    }
}

fn method_from_descriptor(method: &MethodDescriptor) -> ProtoMethodInfo {
    ProtoMethodInfo {
        name: method.name().to_string(),
        request_type: method.input().full_name().to_string(),
        response_type: method.output().full_name().to_string(),
        client_streaming: method.is_client_streaming(),
        server_streaming: method.is_server_streaming(),
        line: 1,
    }
}

fn find_service_by_suffix(pool: &DescriptorPool, service_name: &str) -> Option<ServiceDescriptor> {
    pool.services().find(|service| {
        service.name() == service_name || service.full_name().ends_with(service_name)
    })
}

#[derive(Clone)]
struct DynamicCodec {
    response: prost_reflect::MessageDescriptor,
}

impl DynamicCodec {
    fn new(response: prost_reflect::MessageDescriptor) -> Self {
        Self { response }
    }
}

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynamicEncoder;
    type Decoder = DynamicDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynamicEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynamicDecoder {
            response: self.response.clone(),
        }
    }
}

struct DynamicEncoder;

impl Encoder for DynamicEncoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|error| tonic::Status::internal(error.to_string()))
    }
}

struct DynamicDecoder {
    response: prost_reflect::MessageDescriptor,
}

impl Decoder for DynamicDecoder {
    type Item = DynamicMessage;
    type Error = tonic::Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        if src.remaining() == 0 {
            return Ok(None);
        }
        DynamicMessage::decode(self.response.clone(), src)
            .map(Some)
            .map_err(|error| tonic::Status::internal(error.to_string()))
    }
}

fn to_string(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            read_workspace,
            read_text_file,
            write_text_file,
            get_git_workspace_info,
            pull_git_branch,
            get_git_changes,
            get_git_file_diff,
            commit_and_push_git,
            analyze_proto,
            get_method_template,
            invoke_grpc
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Protohub");
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_request_template_from_method_descriptor() {
        let dir = tempfile::tempdir().expect("tempdir");
        let proto = dir.path().join("voice.proto");
        fs::write(
            &proto,
            r#"
syntax = "proto3";
package vemus.asset.activity;

service VoiceMessageService {
  rpc SubmitVoiceMessage (SubmitVoiceMessageReq) returns (SubmitVoiceMessageRsp);
}

message SubmitVoiceMessageReq {
  string user_id = 1;
  repeated int64 track_ids = 2;
  VoicePayload payload = 3;
}

message VoicePayload {
  bytes data = 1;
  bool urgent = 2;
}

message SubmitVoiceMessageRsp {
  string message = 1;
}
"#,
        )
        .expect("write proto");

        let template = get_method_template_inner(MethodTemplateInput {
            file_path: proto.to_string_lossy().to_string(),
            include_paths: vec![dir.path().to_string_lossy().to_string()],
            service: "vemus.asset.activity.VoiceMessageService".to_string(),
            method: "SubmitVoiceMessage".to_string(),
        })
        .expect("template");

        assert_eq!(
            template.grpc_path,
            "/vemus.asset.activity.VoiceMessageService/SubmitVoiceMessage"
        );
        assert_eq!(
            template.request_type,
            "vemus.asset.activity.SubmitVoiceMessageReq"
        );
        assert_eq!(template.request_json["user_id"], Value::from(""));
        assert_eq!(template.request_json["track_ids"], serde_json::json!(["0"]));
        assert_eq!(
            template.request_json["payload"]["urgent"],
            Value::from(false)
        );
        assert_eq!(template.request_fields.len(), 3);
        assert_eq!(template.request_fields[0].number, 1);
        assert_eq!(template.response_fields.len(), 1);
    }

    #[test]
    fn lightweight_parser_keeps_methods_after_rpc_option_blocks() {
        let analysis = parse_proto_lightweight(
            "voice.proto",
            r#"
syntax = "proto3";
package demo;

service VoiceService {
  rpc First (FirstReq) returns (FirstRsp) {
    option (demo.http) = {
      post: "/first"
    };
  }
  rpc Second (SecondReq) returns (SecondRsp);
}

message FirstReq {}
message FirstRsp {}
message SecondReq {}
message SecondRsp {}
"#,
        );

        let service = analysis
            .services
            .iter()
            .find(|service| service.name == "VoiceService")
            .expect("service");
        assert_eq!(service.methods.len(), 2);
        assert_eq!(service.methods[0].name, "First");
        assert_eq!(service.methods[1].name, "Second");
        assert_eq!(service.methods[1].line, 11);
    }

    #[test]
    fn descriptor_services_are_filtered_to_current_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let imported = dir.path().join("imported.proto");
        let current = dir.path().join("current.proto");
        fs::write(
            &imported,
            r#"
syntax = "proto3";
package demo;
service ImportedService {
  rpc Imported (ImportedReq) returns (ImportedRsp);
}
message ImportedReq {}
message ImportedRsp {}
"#,
        )
        .expect("write imported");
        fs::write(
            &current,
            r#"
syntax = "proto3";
package demo;
import "imported.proto";
service CurrentService {
  rpc Current (CurrentReq) returns (CurrentRsp);
}
message CurrentReq {}
message CurrentRsp {}
"#,
        )
        .expect("write current");

        let pool = compile_descriptor(
            &current.to_string_lossy(),
            &[dir.path().to_string_lossy().to_string()],
        )
        .expect("descriptor");
        let services = services_from_pool(&pool, &current.to_string_lossy());

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].name, "CurrentService");
    }

    #[test]
    fn git_status_parser_preserves_first_path_character() {
        let changes = parse_git_changes(
            " M vemus/asset/activity/voice_msg_service.proto\n?? new.proto",
            Path::new("/repo"),
        );

        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].status, "M");
        assert_eq!(
            changes[0].relative_path,
            "vemus/asset/activity/voice_msg_service.proto"
        );
        assert_eq!(changes[1].status, "??");
        assert_eq!(changes[1].relative_path, "new.proto");
    }

    #[test]
    fn protocol_source_extraction_keeps_rpc_options_and_enums() {
        let source = r#"
enum TaskStatus {
  PROCESSING = 0;
  SUCCESS = 1;
}
service Demo {
  rpc Get (Req) returns (Rsp) {
    option (trpc.api.http) = {
      post: "/demo"
      body: "*"
    };
  }
}
"#;
        let rpc = extract_rpc_source(source, "Get");
        assert!(rpc.contains("option (trpc.api.http)"));
        assert!(rpc.contains("post: \"/demo\""));
        let enums = extract_named_blocks(source, "enum");
        assert!(enums.contains("enum TaskStatus"));
        assert!(enums.contains("SUCCESS = 1"));
    }

    #[test]
    fn imported_enum_collection_follows_only_dependency_graph() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("common");
        fs::create_dir(&nested).expect("mkdir");
        fs::write(
            dir.path().join("root.proto"),
            "import \"common/shared.proto\";\nenum RootState { ROOT = 0; }",
        )
        .expect("root enum");
        fs::write(
            nested.join("shared.proto"),
            "enum SharedState { SHARED = 0; }",
        )
        .expect("shared enum");
        fs::write(
            dir.path().join("unrelated.proto"),
            "enum UnrelatedState { UNRELATED = 0; }",
        )
        .expect("unrelated enum");

        let output = collect_imported_enums(
            &[dir.path().to_string_lossy().to_string()],
            &dir.path().join("root.proto").to_string_lossy(),
        );
        assert!(output.contains("// Source: root.proto"));
        assert!(output.contains("enum RootState"));
        assert!(output.contains("// Source: common/shared.proto"));
        assert!(output.contains("enum SharedState"));
        assert!(!output.contains("UnrelatedState"));
    }
}

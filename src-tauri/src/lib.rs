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
    request_fields: Vec<FieldInfo>,
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
    status: String,
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
    Ok(output
        .lines()
        .filter_map(|line| {
            if line.len() < 4 {
                return None;
            }
            let status = line[..2].trim().to_string();
            let display_path = line[3..]
                .rsplit_once(" -> ")
                .map(|(_, destination)| destination)
                .unwrap_or(&line[3..])
                .trim_matches('"')
                .to_string();
            Some(GitChange {
                path: repository_path
                    .join(&display_path)
                    .to_string_lossy()
                    .to_string(),
                relative_path: display_path,
                status,
            })
        })
        .collect())
}

fn git_output(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("Unable to run git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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

    Ok(MethodTemplateOutput {
        request_type: request_descriptor.full_name().to_string(),
        response_type: response_descriptor.full_name().to_string(),
        request_json: message_template_json(&request_descriptor, 0),
        request_fields: message_fields(&request_descriptor, 0),
        grpc_path,
        client_streaming: method.is_client_streaming(),
        server_streaming: method.is_server_streaming(),
    })
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
        json_name: field.name().to_string(),
        field_type: field_type_label(field),
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
            analyze_proto,
            get_method_template,
            invoke_grpc
        ])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("ProtoHub");
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
}

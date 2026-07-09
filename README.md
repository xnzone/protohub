# ProtoHub

ProtoHub 是一个面向 protobuf 和 gRPC 调试的桌面应用。它基于 Tauri 2、React、Vite 和 Monaco Editor 构建，提供 `.proto` 文件浏览、编辑、结构分析和一元 gRPC 请求测试能力。

## 功能

- 打开本地工作区并浏览 `.proto` 文件树
- 使用 Monaco Editor 编辑 protobuf 文件，支持语法高亮和保存
- 解析 package、service、rpc、message、enum 等结构信息
- 通过 protobuf descriptor 生成请求 JSON 模板和字段树
- 调用一元 gRPC 方法，支持 metadata、TLS 开关、自定义 authority 和 endpoint 变量
- 使用 `{{VARIABLE_NAME}}` 形式管理常用环境地址

## 技术栈

- 前端：React 18、TypeScript、Vite、Monaco Editor、lucide-react
- 桌面端：Tauri 2
- 后端：Rust、prost、prost-reflect、protox、tonic

## 开发环境

需要先安装：

- Node.js
- Rust
- Tauri 2 所需系统依赖

可参考 Tauri 官方文档配置本机环境：<https://tauri.app/start/prerequisites/>

## 安装依赖

```bash
npm install
```

## 本地开发

启动 Tauri 桌面开发模式：

```bash
npm run tauri dev
```

只启动前端开发服务：

```bash
npm run dev
```

前端开发服务默认运行在 `http://127.0.0.1:1420`。

## 构建

构建前端资源：

```bash
npm run build
```

构建桌面应用安装包：

```bash
npm run tauri build
```

## 目录结构

```text
.
├── src/                    # React 前端代码
│   ├── App.tsx             # 主界面、编辑器和 gRPC 测试逻辑
│   ├── main.tsx            # 前端入口
│   └── styles.css          # 应用样式
├── src-tauri/              # Tauri/Rust 后端
│   ├── src/lib.rs          # 文件读写、protobuf 分析和 gRPC 调用命令
│   ├── tauri.conf.json     # Tauri 应用配置
│   └── Cargo.toml          # Rust 依赖配置
├── tools/                  # 辅助脚本
├── package.json            # 前端依赖和脚本
└── vite.config.ts          # Vite 配置
```

## 使用提示

1. 启动应用后点击工作区按钮选择包含 `.proto` 文件的目录。
2. 在编辑器中选择或编辑 protobuf 文件，右侧会展示服务、方法和类型结构。
3. 切换到测试视图，选择 service/method，填写 endpoint、metadata 和请求 JSON。
4. endpoint 支持环境变量模板，例如 `{{LOCAL_HOST}}`，变量会保存在浏览器本地存储中。

当前版本支持普通一元 gRPC 调用；流式 RPC 会被识别，但暂未实现调用。

# Driver Doctor

Windows 磁盘空间分析工具，类似 FolderSize：扫描文件/文件夹大小、按占用排序，并集成 AI 给出清理建议。

## 功能

- **目录扫描**：选择任意文件夹，列出子项及递归大小，默认按大小降序
- **全盘扫描**：扫描所有本地磁盘根目录下的一级文件夹，找出占用最大的目录
- **排序**：点击表头按名称、大小、路径排序
- **资源管理器**：双击条目在 Windows 资源管理器中打开
- **AI 文件夹分析**：说明文件夹用途、是否可清理、具体操作（如微信存储空间清理入口）
- **AI 清理计划**：基于扫描结果生成优先级清理方案

## 环境要求

- Windows 10/11
- [Rust](https://rustup.rs/)（本机已安装即可直接编译）

## 构建与运行

```powershell
cd d:\workspace\driver-doctor
cargo run --release
```

首次编译会下载依赖，可能需要几分钟。

## AI 配置

点击右上角 **设置**，填写 OpenAI 兼容 API：

| 字段 | 说明 |
|------|------|
| Base URL | 如 `https://api.openai.com/v1`、`https://api.deepseek.com/v1` |
| API Key | 你的密钥 |
| Model | 如 `gpt-4o-mini`、`deepseek-chat` |

配置保存在：`%APPDATA%\driver-doctor\config.toml`

本地 Ollama 示例：

- Base URL: `http://localhost:11434/v1`
- API Key: 任意非空字符串（如 `ollama`）
- Model: `llama3` 等

## 使用说明

1. 输入路径或点击「浏览」选择文件夹
2. 点击「扫描」查看该目录下各项占用
3. 点击「全盘扫描」快速了解各盘最大文件夹（适合生成清理计划）
4. 选中一项 →「AI 分析选中项」获取说明与建议
5. 扫描完成后 →「生成清理计划」获取 AI 整理的清理方案

## 技术栈

- Rust + egui/eframe（原生 GUI）
- jwalk（并行目录遍历）
- reqwest（OpenAI 兼容 Chat Completions API）

## 许可证

MIT

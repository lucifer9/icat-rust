[English](README.md)

# icat

`icat` 可以在支持 Kitty 图形协议的终端中直接显示图片和文档内容。它支持图片文件、Markdown、PDF、包含图片的压缩包、Shell glob 模式以及标准输入。

## 功能

- 显示常见图片格式：PNG、JPEG、GIF、BMP、WebP 和 TIFF。
- 渲染 Markdown 标题、列表、表格、引用块、带样式的行内文本、语法高亮代码、本地图片、LaTeX 数学公式和 Mermaid 图表。
- 安全换行较长的 Markdown 内容，同时保留行内代码空白、Unicode 空格、链接下划线和斜体样式。
- 提取可读的 PDF 文本，包括通过内嵌 CMap 恢复的中日韩文本；对于图片型 PDF，则显示其中最大的可解码图片。
- 从 ZIP、TAR、TAR.GZ、7z 和 RAR 压缩包中选择图片。
- 在 tmux 中可用时使用 Kitty passthrough。

## 环境要求

- 带有 Cargo 的 Rust 工具链。
- 输出图片时，需要支持 Kitty 图形协议的终端。

## 构建

```sh
cargo build
```

构建优化后的本地二进制文件：

```sh
cargo build --release
```

Release 二进制文件位于 `target/release/icat`。

## 用法

```sh
cargo run -- image.png
cargo run -- '*.jpg'
cargo run -- document.pdf
cargo run -- README.md
cat README.md | cargo run -- --markdown
cargo run -- -p3 document.pdf
cargo run -- photos.zip
cargo run -- -p 2 photos.zip
cat image.png | cargo run --
```

安装后或 Release 版本的二进制文件使用相同参数，但无需添加 `cargo run --`：

```sh
icat image.png
icat --markdown notes.md
icat --md-font-size 20 README.md
icat -p 2 archive.zip
```

运行 `icat --help` 查看完整选项列表。

## 输入选择

对于 PDF，`-p N` 选择从 1 开始计数的页码。对于压缩包，`-p N` 选择从 1 开始计数的图片序号；不指定 `-p` 时，`icat` 会从压缩包中随机选择一张图片。对于 Markdown，`-p N` 选择渲染后的页面，`--md-font-size N` 设置以磅为单位的基础字号。

当 Markdown 内容跨越多个终端页面、标准输出为交互式终端且未指定 `-p` 时，程序会打开一个简单的分页器。按 Enter 进入下一页，输入页码后按 Enter 跳转，输入 `q` 退出。

使用 `-` 显式读取标准输入：

```sh
cat image.png | icat -
cat notes.md | icat --markdown -
```

## Markdown 渲染

Markdown 图片既可以独占一行，也可以与文本混排行内显示。相对图片路径以 Markdown 文件所在目录为基准；通过标准输入传入 Markdown 时，则以当前工作目录为基准。缺失的行内图片不会导致其前后的文本消失。

数学公式使用 Markdown 支持的标准行内和块级分隔符。语言标记为 `mermaid` 的围栏代码块会渲染为图表。内置 Mermaid 渲染器支持流程图、时序图、类图、状态图和 ER 图。

## PDF 行为

未指定 `-p` 时，`icat` 优先提取整个文档中的可读文本；如果无法获得足够的文本，则回退到第一页中的内嵌图片。指定 `-p N` 时，程序会先尝试显示该页最大的内嵌图片，再回退到该页文本。如果最大的图片已损坏或不受支持，则继续尝试下一张较小的可解码图片。

## 开发

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo test --locked
```

单元测试与对应模块放在一起，CLI 端到端测试位于 `tests/cli.rs`。

## 许可证

本项目采用 MIT 许可证，详情请参阅 `LICENSE`。

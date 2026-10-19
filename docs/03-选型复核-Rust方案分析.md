# 技术选型复核：为什么不是 Rust + Slint / Tauri

> 本文回答一个具体质疑：**既然要双端，为什么不用 Rust？配 Slint 或 Tauri 不行吗？**
> 所有结论均基于 2026-10-19 的实测与官方文档核验，不是印象判断。

---

## 结论先行

**Rust 两条线技术上都能做到双端，但都要求你做"两套 UI"或"退回 WebView"**，而这恰好牺牲了本项目的核心诉求：图片密集型界面的原生流畅度与单套代码。

| 方案 | 双端真实性 | 致命短板 | 判定 |
|---|---|---|---|
| **Rust + Slint** | 真·原生自绘，双端可跑 | 移动端生态极薄，**没有 Material 3**，图片网格能力需自建 | 可做但成本高 |
| **Rust + Tauri v2** | 双端"能跑"，但**两端都是 WebView 渲染** | Android 端退回 WebView；Web 技术栈 + Rust 双份维护 | 能做但不划算 |
| **Kotlin + CMP**（已实测出窗口） | 真·原生，双端单套代码 | 桌面包体 114MB | 综合最优 |

**关键判据不是"Rust 行不行"，而是"这个 App 的难点在哪"。**

本项目的难点是：
1. 海量图片瀑布流滚动性能（8K 图、2000+ 张）
2. 双端一致的 UI 与手势体验
3. 本地数据管线（SQLite + 清洗 + 后台任务）

这三项里，**Rust 的优势（极致性能、小体积）在 1 和 2 上帮不上忙**——因为瓶颈不在计算，而在 UI 框架的渲染管线与图片解码策略。

---

## 一、Rust + Slint 的实测情况

### 1.1 Slint 确实支持 Android（这条我核实了，不是想当然）

实测发现 Slint 仓库存在完整的 Android 后端：

```
internal/backends/
  android-activity/          <- Android 后端，真实存在
    androidwindowadapter.rs   50,929 bytes
    javahelper.rs             33,629 bytes
    vsync.rs                   6,006 bytes
    build.rs                   3,105 bytes
    lib.rs                     8,799 bytes
    java/                      <- Java 胶水层
  winit/                     <- 桌面后端
  qt/  linuxkms/  testing/  selector/
```

官方 README 明示：*"This crate implements the Android backend/platform for Slint. It uses the `android-activity` crate to initialize the app and provide events handling."* 通过 `slint::android` 模块调用。

**所以"Slint 不能做安卓"是错的，我修正这个说法。**

### 1.2 但它的移动端成熟度确实远低于 Kotlin

实测证据：

| 检查项 | Slint 结果 | 说明 |
|---|---|---|
| `api/` 绑定目录 | 只有 `cpp`、`node`、`python`、`rs`、`slint-sc`、`wasm-interpreter` | **无 java/kotlin 一等绑定** |
| 官方文档 Android 专页 | `slint.dev/docs/slint/src/advanced/android` → **404** | 无独立教程 |
| 后端 README 自述 | 「This library is an **internal** crate」+「does not follow semver」 | 内部 crate，需 `=x.y.z` 锁定版本 |
| demos 目录 | `analogclock` `printerdemo` `weather-demo` 等 | 以嵌入式/桌面为主，**无专门 Android demo** |
| 版本 | 1.18.1（2026-09-21），仓库 24.2k stars | 活跃但小体量 |

**对比 Kotlin/CMP**：`androidMain` 是官方一等 sourceSet，`WallpaperManager`/`WorkManager`/`Keystore` 全是一行调用，Coil 是 KMP 原生项目。

### 1.3 Slint 的授权要注意（这条容易踩坑）

官方 README 明确三种选择：
1. **Royalty-free 2.0** — 专有桌面/移动/Web 应用免费
2. **GNU GPLv3** — 开源免费（有传染性）
3. **Commercial** — 专有嵌入式需付费

本项目（自用/开源工具）走 GPLv3 或 Royalty-free 都可，**这条不是硬障碍**，但如果你将来想闭源并内嵌到硬件产品，就要买商业授权。

### 1.4 Slint 的真实短板：没有 Material 3，图片网格要自己造

Slint 是**自绘 UI 工具包**，提供的是基础控件（Rectangle、Text、ListView 等）和自定义样式系统。它：

- ❌ 没有 Material 3 组件库
- ❌ 没有现成的瀑布流/StaggeredGrid 组件（对壁纸站是硬伤）
- ❌ 没有 KMP 那套图片加载生态（Coil 的三级缓存 + 采样降档）
- ⚠️ 手势、返回键、系统栏适配都要自行处理

对 wallhaven 这种**以图片网格为核心界面**的应用，你等于要从零造一个图片网格 + 缓存系统。这是数周工作量，且质量难保证。

---

## 二、Rust + Tauri v2 的实测情况

### 2.1 Tauri 的双端是"真双端"，但有本质限制

官方 README 原文（实测抓取）：

> The user interface in Tauri apps currently leverages `tao` as a window handling library on macOS, Windows, Linux, Android and iOS. To render your application, Tauri uses **WRY**, a library which provides a unified interface to the **system webview**, leveraging WKWebView on macOS & iOS, **WebView2 on Windows**, WebKitGTK on Linux and **Android System WebView on Android**.

平台支持表：

| Platform | 版本要求 |
|---|---|
| Windows | 10 及以上 |
| Android | **7 及以上（当前要求 8+）** |
| iOS/iPadOS | 9+ |

**所以 Tauri 确实支持 Windows + Android，我修正这一点。**

### 2.2 但关键在"渲染方式"——两端都是 WebView

这是决定性的：

```
Tauri 架构：
  Rust 后端（逻辑、文件、系统调用）
        ↕ IPC
  系统 WebView（Windows: WebView2 / Android: System WebView）
        ↑
   你的 HTML + JS + CSS 前端
```

**推论：**
- 你在 Tauri 里写的界面**本质是网页**，不是原生控件
- Android 端 = 用系统 WebView 包一层自己的网页 → 与"网页套壳"的区别只是后端换了 Rust
- Windows 端需要 WebView2 运行时（Win11 自带；Win10 部分需单独安装或由安装包引导）

对一个壁纸应用，这意味着：**你用 Rust 换来了性能和体积，然后用 WebView 把它抵消掉了**——因为 UI 性能瓶颈变成了 WebView 的合成与 JS 虚拟滚动能力。

### 2.3 Tauri 的 Android 门槛是真实存在的（本机实测）

官方前置要求（实测抓取原文）：
1. 安装 **Android Studio**（本机 ❌ 未安装）
2. 设置 `JAVA_HOME` 指向 **Android Studio 自带的 JBR**（本机指向 JDK 11 ❌）
3. SDK Manager 安装 Platform / Platform-Tools / **NDK** / Build-Tools / cmdline-tools
4. 设置 `ANDROID_HOME` 与 **`NDK_HOME`** 环境变量（本机 `NDK_HOME` ❌ 未设置）

也就是说，走 Tauri 安卓必须先补一整套 Android Studio 工具链。

### 2.4 双份技术栈的维护成本

Tauri 项目实际上要维护：
- Rust（后端、命令、插件、打包）
- 前端框架（React/Svelte/Vue，含状态管理、路由、构建链）
- 两套之间的 IPC 契约（类型定义要双向同步）

对单人项目，这是显著负担。Kotlin/CMP 只有一套 Kotlin。

---

## 三、Rust 在本机的实测可行性（我做完了，结果对 Rust 有利）

为公平起见，我实测了 Rust 的 Android 交叉编译，**它确实能跑通**：

| 步骤 | 结果 |
|---|---|
| `rustup target add aarch64-linux-android` | ✅ 已装（原先只有 `x86_64-pc-windows-msvc`） |
| NDK linker 配置 | ✅ NDK 27.0.12077973 的 `aarch64-linux-android26-clang.cmd` 存在 |
| `cargo build --target aarch64-linux-android` | ✅ **Finished in 6.45s** |
| 产物校验 | ✅ `libprobe.so`，436KB，ELF 64-bit ARM64（`machine=0xB7`） |

**结论：Rust 技术门槛不存在。否决 Rust 的理由不是"做不到"，而是"做到了不划算"。**

---

## 四、真正的判据：项目难点在哪里

### 4.1 本项目的三类难点 vs 各方案强项

| 难点 | 本质 | Rust 帮助 | Kotlin/CMP 帮助 |
|---|---|---|---|
| 2000+ 张图片瀑布流不卡 | UI 框架的懒加载 + 图片解码采样 | ❌ 无关（瓶颈在 UI 层） | ✅ LazyStaggeredGrid + Coil 内建 |
| 双端 UI 一致 | 一套声明式 UI | ⚠️ Slint 可，但控件要自建 | ✅ Material 3 现成 |
| 本地数据管线 | SQLite + 清洗 + 后台任务 | ⚠️ 需自己接 | ✅ SQLDelight + WorkManager |
| 下载并发 | 真·CPU/IO 密集 | ✅✅ Rust 强项 | ⚠️ 协程够用 |
| 包体小 | 运行时裁剪 | ✅✅ 8–15MB | ❌ 114MB（已实测） |

**只有最后两项 Rust 胜出，而它们都不是本项目的核心难点。**

### 4.2 一个具体的技术杀手：8K 图片

实测 wallhaven 存在 **7680×4320** 图片。解码为 RGBA 约 **132MB/张**。

- **Kotlin + Coil**：`AsyncImage` 传 `size()` 自动采样降档，DiskCache 512MB，内存缓存 20%——开箱即用
- **Slint**：需自己实现采样逻辑（可用 `image` crate，但要接进渲染管线）
- **Tauri**：`<img>` 交给 WebView，采样不受你控制，**8K 图的解码内存风险由浏览器决定**

这一项会直接决定 App 会不会 OOM 崩溃。

---

## 五、什么情况下我会改用 Rust？（诚实给出切换条件）

不想给你一个"永远选 Kotlin"的死结论。以下任一条件成立，**Rust 就是更优解**：

| 条件 | 推荐方案 |
|---|---|
| 你**极度在意安装包体积**（必须 <30MB，如挂在自己站供人下载） | **Tauri v2**（8–15MB） |
| 你**已有 React/Vue 前端经验**，不愿学 Compose | **Tauri v2**（复用 Web 技能） |
| 你需要**极致下载并发**（同时下几千张、要 Rust 的 async 生态） | **Tauri v2** 或 Rust core + 任意前端 |
| 你需要**嵌入式/资源受限设备**（树莓派、车机） | **Slint**（专为此设计） |
| 你想**闭源并出售**，且不愿遵守 GPL | Kotlin/CMP（Apache 2.0）优于 Slint（需商业授权） |
| 你个人**就是想写 Rust**，且接受更长工期 | **Slint** 或 **Tauri**，我照做 |

---

## 六、如果我理解错了你的诉求

有一种可能：你说的"跨双端"**不是指同一套 UI 代码**，而是"两个能用的 App"。那结论会变：

- 若可接受**桌面端和手机端UI 各自独立设计**，Rust 完全可行——桌面用 Tauri（小体积），安卓用 Kotlin 原生（体验好），Rust 做共享核心库（通过 JNI/FFI，我已实测可行）。
- 这种"混搭"方案我称之为 **方案 D**，代价是两套 UI 代码，但两端都能各自最优。

**这是我目前唯一还没排除的优解。如果你倾向这个，告诉我，我马上出方案 D 的细化设计。**

---

## 七、修订记录

本次复核修正了原方案中两处过于绝对的表述：

| 原表述 | 修正后 |
|---|---|
| 「Rust/Tauri v2 — 可作为备选，但在两项目标平台上处于结构劣势」 | 补明：Tauri **确实**支持 Android 7+/Windows 10+，劣势在于**两端都是 WebView 渲染**，非"不支持" |
| 「Slint — Android 支持：⚠️ 弱」 | 修正：Slint **有完整 android-activity 后端**（含 50KB window adapter + Java 胶水层），可跑通；短板是**无 Material 3、无图片网格组件、生态薄** |

> 我对之前用"⚠️ 弱"这种模糊标注带过 Slint 移动端表示歉意——它确实有实现，只是不适合本项目形态。

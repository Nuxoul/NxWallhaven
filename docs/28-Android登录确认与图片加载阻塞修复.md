# Android alpha04：登录确认等待与图片缓存阻塞修复

## 反馈与结论

用户在实际手机报告：应用内网页完成登录，点击上方“已完成，核对登录”返回原生页后长时间不能确认；同一网络下手机图片加载明显慢。

本轮没有连接用户手机，也没有其 Android/WebView 版本或现场计时，**不能声称完整复现用户真实账号/网络环境或确认所有问题已消失**。已建立并修复两个确定的应用内故障模式，交付 alpha04 供升级复测。未索取或导出密码、Cookie、API Key，没有自动登录/写入真实账号。

## 1. 登录网页的图片没加载完，会阻塞原生核对

### 实际失败回归

新增 `WebsiteSessionTest.loginDoneRecognizesSessionWithoutWaitingForPageImages`：使用真正 Android WebView、HttpOnly 测试登录、实际原生“已完成”按钮和 AccountController/Rust 账号解析；登录成功页包含一个延迟结束的图片资源。

未修复代码中，点击按钮后 3 秒仍没有识别用户名：

```text
login confirmation waited for an unrelated page image instead of recognizing the authenticated session
expected:<FixtureUser> but was:<null>
```

该用例通过宿主命令真实运行失败，不是只读代码猜测：

```powershell
python .\scripts\test-android-emulator.py --class 'io.github.nuxoul.nxwallhaven.WebsiteSessionTest#loginDoneRecognizesSessionWithoutWaitingForPageImages'
```

### 原因和修复

`WebSession.request()` 原先只在 `onPageFinished` 后才启动请求；网页主文档和登录会话已可用，但一张慢图片/其他子资源使页面仍处于 loading，原生核对就被无关资源拖住。

现在利用 `onPageCommitVisible` 记录新主文档已经提交；在相同来源、主文档提交后即可执行会话请求，不再等待所有图片。主导航开始清除就绪状态并使旧任务失效；显式清理/销毁也重置状态。仍核对来源、导航代次和 SSL 错误，Cookie 仍只在 WebView 内；没有重用旧页面身份、绕过验证或自动重试 POST。

同一回归修复后通过；完整网站本地夹具 5 项及跨进程恢复/清理也通过。原生账号页现在区分“核对会话”和“已登录、正在读取收藏夹”，显示等待进度与超时提示；没有以收藏读取尚未结束掩盖已识别的账号。

## 2. 已缓存大原图校验会挡住其他图片

### 可复现瓶颈

缓存命中需要核对文件长度和 SHA-256，旧版在整个文件读取/哈希期间持有缓存目录的全局磁盘锁。某张大原图在较慢手机存储上校验时，不相关的缩略图命中和新 GET 的准备都要排队。

带受控磁盘校验延迟的宿主回归在旧版真实失败：

```text
unrelated_hit=1.1107587s unrelated_miss=1.3221635s
unrelated_hit_before_release=false unrelated_miss_before_release=false miss_GETs_before_release=0
full-file hit verification blocked unrelated cache hit/GET behind the disk mutex
```

修复把完整文件哈希移出全目录锁，只在打开前/更新元数据前短暂加锁；清缓存仍独占原有 clear gate，活跃按 URL flight 仍避免 LRU 淘汰。保留长度/SHA 校验、损坏拒绝、文件/链接/目录归属校验，以及原图/下载的共享 GET 和续传语义。

相同受控回归：不相关命中 **10.811ms**、未命中 **24.2746ms**，在慢校验释放前均完成，未命中 GET 已启动。另有同长度损坏、清理独占和活跃文件淘汰保护、单连接复用回归通过。这是宿主注入延迟的锁竞争测量，**不是用户手机冷加载提速百分比**。

## 3. 模拟器真实网络分段计时

保留测试专用 `ImageTimingProbe`，使用独立缓存、公开 SFW 图片 `w5mqr6`，记录阶段/时间/字节/尺寸，不记录 Cookie、认证头、Key、任意 URL 或网页正文。对比 Android 正常 HttpsURLConnection 与 Rust 路径；系统客户端仅在测试中使用，没有替换正式下载实现或降低证书校验。

一次最终成功的 Android 36 x86_64 专用模拟器测量：

| 阶段 | 耗时 | 数据 |
| --- | ---: | --- |
| 缩略图冷获取 | 2819ms | 18,677 字节 |
| 缩略图缓存命中 | 7ms | 相同文件 |
| 系统 HTTPS 原图 | 8586ms | 响应头等待 530ms；4,315,271 字节 |
| Rust 原图冷获取（含落盘/校验） | 10197ms | 4,315,271 字节 |
| Rust 原图缓存命中 | 19ms | 相同文件 |
| 本地原图首批图块显示 | 356ms | 完整 6240×4160 |

另一轮系统 HTTPS 5727ms、Rust 7768ms；更早一轮原图获取出现 Network 错误，尚未进入图块解码，不能隐藏这次失败。临时计时探针的 URI/Compose 测试帧等待写法不一致也曾导致误报图块超时，已修正为正式查看器的 file URI 并推进 Compose 帧，未因此改动正式查看器。

因此当前证据是：**已确认并移除缓存锁造成的跨图片阻塞；冷加载仍明显受获取阶段影响，不能仅凭同 Wi-Fi 推断手机与电脑应相同，也不能据单次模拟器顺序测试把剩余问题归因于网络或 CPU。** 用户设备的 DNS/IPv6、VPN/代理、私有 DNS、CDN、WebView 版本及解码耗时尚未测得。真实手机后续需提供型号/Android 版本、慢的是缩略图还是原图，以及大致冷/热耗时，不需要提供凭据。

原图没有降分辨率、改成缩略图显示、固定响应大小限制或 2000px 降采样；没有更换来源、UA 伪装或绕过 TLS。

## 验证

- Rust 全工作区串行 **233 项通过**：Android 34 / core 42 / desktop 121 / site 30 + 集成 6；既有 7 项忽略未计通过。
- Rust fmt、严格全工作区 Clippy 通过；Kotlin 6 项单元通过；Lint 0 错误 / 41 警告。
- 最终全套模拟器 **19 项测试/步骤通过**：浏览 4、下载/图库 4、下载重启 2、网站夹具 5、网站重启 2、缓存重启 2。
- 单独真实网络图片计时 1 项成功（不并入确定性 19 项）；前述失败与波动明确保留。
- 主线检查本次 diff、构建和实际回归，没有额外发起无限可选审查；本轮性能任务只修改 cache 两文件，其宿主结果随后由主线全套和 Android 构建复验。
- 自建模拟器运行后恢复网络并退出；未触碰真实设备/账号写入/壁纸设置。

## 升级包

- 大小 **45,954,906 字节**；SHA-256：`6D939889CAD1712109D9A69234AB235CF3BEFC9BADBE6F2DBE2ABA6CA603409A`。
- APK 构建产物与 dist 副本哈希一致，apksigner 验证有效；`zipalign -c -P 16 4` 与全部 6 个 ELF LOAD 段静态 16KB 对齐通过，未在 16KB 真实设备验收。仍是 Debug 测试包、不是生产签名。
- versionCode 4，Android 8+，ARM64/x86_64，仍使用与 alpha01–03 相同 Debug 证书。**直接覆盖升级，不要先卸载/清数据**，避免丢失现场会话和配置。
- 包路径：`dist/android/NxWallhaven-0.1.0-alpha04-debug.apk`。

## 后续复测

1. 覆盖升级后打开“收藏→网站登录”，在应用内完成登录及网站验证，等页面显示已登录状态后点“已完成，核对登录”。
2. 看原生页具体停在“核对会话”还是“已登录、读取收藏夹”，记录提示文本和大致等待时长。
3. 分别试列表缩略图、打开原图、返回后重开同图；记录哪一步慢。不要为测试清除全部数据。
4. 若仍异常，提供手机型号、Android 版本、Android System WebView/Chrome 版本、是否使用 VPN/代理/私有 DNS，以及不含账号敏感内容的状态截图。真实账号登录仍需用户本人验收。

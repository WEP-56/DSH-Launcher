<p align="center">
  <img src="assets/dsh-icon-source.png" alt="DSH Launcher 图标" width="180">
</p>

# DSH Launcher

一个基于 Tauri 2 的 DeepSeek Harness 桌面启动器。它负责启动和停止 `dsh web`，主窗口只承载 dsh WebUI；包管理、配置文件编辑、插件安装和新窗口入口都收纳在窄工具栏的弹窗里。

插件目录使用 [DSH 插件商店](https://dsh.aitreez.com/) 的公开页面，仅在用户打开插件弹窗时加载一次；安装通过 dsh 官方命令完成，例如 `dsh plugin --profile web add github:omdsh-dev/DSH-better-sidebar`。

## 界面截图

<p align="center">
  <img src="assets/image1.png" alt="DSH Launcher 主界面" width="32%">
  <img src="assets/image2.png" alt="DSH Launcher 设置界面" width="32%">
  <img src="assets/image3.png" alt="DSH Launcher 插件界面" width="32%">
</p>

## 开发

需要 Node.js 22+、Rust 和系统 WebView2：

```powershell
npm install
npm run tauri dev
```

启动器默认使用 PATH 中的 `dsh`：

```powershell
npm install -g @deepseek-ai/dsh
```

也可以在启动设置中切换为 npx 模式。首次通过 npx 启动需要网络连接。

## 构建

```powershell
npm run tauri build
```

配置保存在 Tauri 的应用配置目录中。退出应用时，启动器会终止由它创建的 dsh 进程树。

## 发布

推送形如 `v*` 的 Git tag 后，GitHub Actions 会在 Windows runner 上构建安装包并创建 GitHub Release。发布产物包括 Tauri 配置中启用的 Windows 安装包。

## 相关链接

- [DSH 插件商店](https://dsh.aitreez.com/)
- [Linux.do](https://linux.do)

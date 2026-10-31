# syncraft-server

Minecraft server synchronisation daemon. The binary is installed as `syncraft`.

## 本地安装 / 更新

在仓库根目录执行（`syncraft-server` 是 workspace 成员，直接按路径安装即可，
`../console_info`、`../syncraft-protocol`、`../mod-type` 这些 path 依赖会自动一起构建）：

```sh
cargo install --path syncraft-server --locked --force
```

- `--locked` 使用仓库里的 `Cargo.lock`，避免安装时解析出与开发环境不同的依赖版本。
- `--force` 是关键：`Cargo.toml` 的版本号没变时，cargo 默认会跳过安装；本地迭代
  （`git pull` 之后、改完代码之后）需要它来覆盖 `~/.cargo/bin/syncraft`。
- 安装目标是 `$CARGO_HOME/bin`（通常是 `~/.cargo/bin`），请确保它在 `PATH` 中。

验证：

```sh
syncraft --version
```

`syncraft serve install` 生成的 systemd 单元使用 `current_exe`，因此重装到同一路径后，
已经注册的服务会直接使用新版本，无需重新执行 `serve install`。

## 指定安装目录

```sh
cargo install --path syncraft-server --locked --force --root ~/.local
# -> ~/.local/bin/syncraft
```

## 备注

- 默认使用 workspace 的 release profile（fat LTO + 单 codegen unit + strip），
  首次安装会编译整个依赖树，比较慢；之后的更新只重建改动过的 crate。
- 卸载：`cargo uninstall syncraft`。

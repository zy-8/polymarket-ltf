# Repository Guidelines

这个文件是仓库级协作入口。  
当前文档体系以 3 份主文档为准：

- [docs/project.md](docs/project.md)
  项目定义、系统设计、架构摘要和路线图摘要
- [docs/research.md](docs/research.md)
  研究方法、数据标准、量化与高频套利核心问题
- [docs/development.md](docs/development.md)
  工程规范、低延迟约束、测试和文档同步标准

这 3 份主文档分别覆盖项目定义、研究方法和开发规范。

## 项目角色

`polymarket-ltf` 是一个面向 **Polymarket crypto 市场研究** 的 LTF 量化研究基础设施。

当前仓库聚焦：

- Rust 实时数据接入
- snapshot 特征生成
- Python 离线回测与批量评估
- 研究、结果和日志标准

当前不应被描述为：

- 完整实盘执行系统
- 完整风控与订单管理平台
- 已上线的自动交易框架

## 进入仓库前先看

- 项目定义、系统设计与阶段方向： [docs/project.md](docs/project.md)
- 研究方法与策略准入清单： [docs/research.md](docs/research.md)
- 工程约束与低延迟开发规范： [docs/development.md](docs/development.md)
- Python 回测说明： [backtest/README.md](backtest/README.md)

## 关键目录

- `src/`
  Rust 实时数据链路核心
- `src/strategy/`
  Rust 侧策略目录，适合放运行时策略或执行候选逻辑
- `examples/`
  最小可运行示例
- `backtest/`
  Python 回测与研究项目
- `docs/`
  正式项目文档
- `skills/`
  项目内 skill

## 必须遵守的规则

- 修改前先理解数据流、模块边界和字段语义。
- 数据源逻辑放独立模块，不要把多交易所逻辑堆到 `main.rs`。
- 离线回测策略放 `backtest/src/strategies/`，Rust 运行时策略放 `src/strategy/`。
- 回测和离线分析逻辑放在 `backtest/` 下，不要混入 Rust 实时层。
- Python 回测入口支持 `--data-format` 和 `--strategy`，新增 loader 或策略时先走注册表。
- 不要把尚未实现的能力写成既成事实。
- snapshot 字段、CLI 行为、目录结构和结果格式变化时，必须同步更新文档。
- 真实数据放 `data/`，不要塞进 `tests/fixtures/`。
- 新增数据源时，先明确标准化口径，再决定是否进入 snapshot。
- 回测、策略、执行和日志必须能追溯到数据、参数和版本。

## 常用验证命令

Rust:

```bash
cargo check
cargo fmt
cargo test
```

Python:

```bash
cd backtest
PYTHONPATH=src python3 -m unittest discover -s tests
PYTHONPATH=src python3 -m backtest --interval 5m
PYTHONPATH=src python3 -m backtest --interval 15m
PYTHONPATH=src python3 -m scan --csv tests/fixtures/sample.csv --top-k 1
```

范围较大时再执行：

```bash
cargo clippy --all-targets --all-features -D warnings
```

## Skills

项目内 skill 位于：

```text
skills/
├── dev/
└── backtest/
```

- `skills/dev/`
  处理 Rust 实时链路和多源数据接入
- `skills/backtest/`
  处理回测、研究输出和策略验证

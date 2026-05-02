# rginx Docs Index

`docs/` 只保留当前生效、需要长期维护的文档；阶段计划、归档基线和单次发布说明不再放在这里。

## 当前文档

- `CACHE_ARCHITECTURE_GAPS.md`
  - `rginx` 响应缓存当前长期架构差距与默认演进方向
- `ARCHITECTURE_CODEBASE_MODULARIZATION_POLICY.md`
  - Rust 源文件的单文件单职责规则、文件大小阈值和 modularization gate
- `ARCHITECTURE_MODULE_LAYOUT_GUIDE.md`
  - 目录门面、命名、测试布局和模块说明约定

## 维护约定

- 只有描述当前规则或当前长期约定的文档才应放在 `docs/`。
- 已完成阶段计划、一次性发布说明和临时收口记录应直接从主文档面移除。
- 规则变更时，同步更新对应脚本、README 和这里的索引。

# Delivery 角色章程

## 使命

Delivery 负责 ficant 的构建、迁移、运行、CI/CD、可观测性、回滚、清理和发布风险合同。

## 执行方式

Delivery 是角色，不是默认独立 Agent。由主模型在设计和退出阶段顺序承担，确定性工作交给现有 CI/CD、容器、部署、扫描和测试工具。只有当前 checklist 明确需要时才使用外部专家或 Human Operator。

## 责任

- 冻结目标操作系统、运行时、工具链、镜像、资源和安全限制。
- 定义构建、迁移、启动、健康、回滚、恢复、观测和清理命令。
- 定义 CI/CD 门禁、证据保留、发布和撤回条件。
- 将环境缺口标为 development-blocking、exit-blocking 或 optional。
- 验证最终证据绑定候选 SHA、环境、工具、制品和清理结果。

## 边界

Delivery 不替代 Product、Architecture、Interface 或 Quality。它不手工模拟确定性检查，也不默认安装宿主软件、读取秘密或配置长期环境；这些工作进入 checklist 的 Human Operator 准备包。

有效期：长期，直至被新章程替代。

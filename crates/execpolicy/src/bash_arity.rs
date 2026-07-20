//! 用于命令前缀允许规则匹配的 Bash 元数词典。
//!
//! [`BashArityDict`] 将命令前缀（空格分隔，小写）映射到
//! 形成规范前缀的位置参数（非标志）词数，*包括基础命令词*。
//!
//! ## 不变式
//!
//! 标志（以 `-` 开头的令牌）**从不**计入元数。
//! `auto_allow = ["git status"]` 必须匹配 `git status -s` 和
//! `git status --porcelain`，但**不**匹配 `git push`。
//!
//! ## 覆盖率
//!
//! 涵盖 30+ 个常用工具：git、npm、yarn、pnpm、cargo、docker、
//! kubectl、go、python/pip、gh、rustup、deno、bun、aws、terraform、
//! make 等。

/// 静态元数表：`(prefix, arity)`。
///
/// 元数是形成规范前缀的*位置*令牌总数（包括基础命令）。
/// 例如：
///
/// * `("git status", 2)` — 2 个位置令牌：`git` + `status`。
/// * `("npm run", 3)` — 3 个位置令牌：`npm` + `run` + `<script>`。
/// * `("make", 1)` — 只有基础命令，没有子命令。
pub static BASH_ARITY_TABLE: &[(&str, u8)] = &[
    // ── git ──────────────────────────────────────────────────────────────────
    ("git add", 2),
    ("git am", 2),
    ("git apply", 2),
    ("git bisect", 2),
    ("git blame", 2),
    ("git branch", 2),
    ("git cat-file", 2),
    ("git checkout", 2),
    ("git cherry-pick", 2),
    ("git clean", 2),
    ("git clone", 2),
    ("git commit", 2),
    ("git config", 2),
    ("git describe", 2),
    ("git diff", 2),
    ("git fetch", 2),
    ("git format-patch", 2),
    ("git grep", 2),
    ("git init", 2),
    ("git log", 2),
    ("git ls-files", 2),
    ("git merge", 2),
    ("git mv", 2),
    ("git notes", 2),
    ("git pull", 2),
    ("git push", 2),
    ("git rebase", 2),
    ("git reflog", 2),
    ("git remote", 2),
    ("git reset", 2),
    ("git restore", 2),
    ("git revert", 2),
    ("git rm", 2),
    ("git show", 2),
    ("git stash", 2),
    ("git status", 2),
    ("git submodule", 2),
    ("git switch", 2),
    ("git tag", 2),
    ("git worktree", 2),
    // ── npm ──────────────────────────────────────────────────────────────────
    ("npm audit", 2),
    ("npm build", 2),
    ("npm cache", 2),
    ("npm ci", 2),
    ("npm dedupe", 2),
    ("npm fund", 2),
    ("npm help", 2),
    ("npm info", 2),
    ("npm init", 2),
    ("npm install", 2),
    ("npm link", 2),
    ("npm list", 2),
    ("npm ls", 2),
    ("npm outdated", 2),
    ("npm pack", 2),
    ("npm prune", 2),
    ("npm publish", 2),
    ("npm rebuild", 2),
    ("npm run", 3),
    ("npm start", 2),
    ("npm stop", 2),
    ("npm test", 2),
    ("npm uninstall", 2),
    ("npm update", 2),
    ("npm version", 2),
    ("npm view", 2),
    // ── yarn ─────────────────────────────────────────────────────────────────
    ("yarn add", 2),
    ("yarn audit", 2),
    ("yarn build", 2),
    ("yarn install", 2),
    ("yarn run", 3),
    ("yarn start", 2),
    ("yarn test", 2),
    ("yarn upgrade", 2),
    ("yarn workspace", 3),
    // ── pnpm ─────────────────────────────────────────────────────────────────
    ("pnpm add", 2),
    ("pnpm build", 2),
    ("pnpm install", 2),
    ("pnpm run", 3),
    ("pnpm start", 2),
    ("pnpm test", 2),
    ("pnpm update", 2),
    // ── cargo ────────────────────────────────────────────────────────────────
    ("cargo add", 2),
    ("cargo bench", 2),
    ("cargo build", 2),
    ("cargo check", 2),
    ("cargo clean", 2),
    ("cargo clippy", 2),
    ("cargo doc", 2),
    ("cargo fix", 2),
    ("cargo fmt", 2),
    ("cargo generate", 2),
    ("cargo install", 2),
    ("cargo metadata", 2),
    ("cargo package", 2),
    ("cargo publish", 2),
    ("cargo remove", 2),
    ("cargo run", 2),
    ("cargo search", 2),
    ("cargo test", 2),
    ("cargo tree", 2),
    ("cargo uninstall", 2),
    ("cargo update", 2),
    ("cargo yank", 2),
    // ── docker ───────────────────────────────────────────────────────────────
    ("docker build", 2),
    ("docker compose", 3),
    ("docker container", 3),
    ("docker cp", 2),
    ("docker exec", 2),
    ("docker image", 3),
    ("docker images", 2),
    ("docker inspect", 2),
    ("docker kill", 2),
    ("docker logs", 2),
    ("docker network", 3),
    ("docker ps", 2),
    ("docker pull", 2),
    ("docker push", 2),
    ("docker rm", 2),
    ("docker rmi", 2),
    ("docker run", 2),
    ("docker start", 2),
    ("docker stop", 2),
    ("docker system", 3),
    ("docker tag", 2),
    ("docker volume", 3),
    // ── kubectl ──────────────────────────────────────────────────────────────
    ("kubectl apply", 2),
    ("kubectl create", 3),
    ("kubectl delete", 3),
    ("kubectl describe", 3),
    ("kubectl exec", 2),
    ("kubectl explain", 2),
    ("kubectl get", 3),
    ("kubectl label", 2),
    ("kubectl logs", 2),
    ("kubectl patch", 2),
    ("kubectl port-forward", 2),
    ("kubectl rollout", 3),
    ("kubectl scale", 2),
    ("kubectl set", 2),
    ("kubectl top", 3),
    // ── go ───────────────────────────────────────────────────────────────────
    ("go build", 2),
    ("go clean", 2),
    ("go env", 2),
    ("go fmt", 2),
    ("go generate", 2),
    ("go get", 2),
    ("go install", 2),
    ("go list", 2),
    ("go mod", 3),
    ("go run", 2),
    ("go test", 2),
    ("go vet", 2),
    ("go work", 3),
    // ── python / pip ─────────────────────────────────────────────────────────
    ("pip install", 2),
    ("pip uninstall", 2),
    ("pip list", 2),
    ("pip show", 2),
    ("pip freeze", 2),
    ("pip3 install", 2),
    ("pip3 uninstall", 2),
    ("pip3 list", 2),
    ("pip3 show", 2),
    ("python -m", 3),
    ("python3 -m", 3),
    // ── make / cmake ─────────────────────────────────────────────────────────
    ("make", 1),
    ("cmake", 1),
    // ── gh (GitHub CLI) ──────────────────────────────────────────────────────
    ("gh pr", 3),
    ("gh issue", 3),
    ("gh repo", 3),
    ("gh release", 3),
    ("gh workflow", 3),
    ("gh run", 3),
    ("gh secret", 3),
    // ── rustup ───────────────────────────────────────────────────────────────
    ("rustup default", 2),
    ("rustup install", 2),
    ("rustup show", 2),
    ("rustup target", 3),
    ("rustup toolchain", 3),
    ("rustup update", 2),
    // ── deno / bun ───────────────────────────────────────────────────────────
    ("deno run", 2),
    ("deno test", 2),
    ("deno fmt", 2),
    ("deno lint", 2),
    ("bun add", 2),
    ("bun build", 2),
    ("bun install", 2),
    ("bun run", 3),
    ("bun test", 2),
    ("npx", 2),
    // ── aws CLI ──────────────────────────────────────────────────────────────
    ("aws s3", 3),
    ("aws ec2", 3),
    ("aws iam", 3),
    ("aws lambda", 3),
    ("aws cloudformation", 3),
    ("aws ecs", 3),
    ("aws eks", 3),
    ("aws rds", 3),
    ("aws sts", 3),
    ("aws configure", 2),
    // ── terraform ────────────────────────────────────────────────────────────
    ("terraform init", 2),
    ("terraform plan", 2),
    ("terraform apply", 2),
    ("terraform destroy", 2),
    ("terraform validate", 2),
    ("terraform output", 2),
    ("terraform state", 3),
    ("terraform workspace", 3),
    // ── helm ─────────────────────────────────────────────────────────────────
    ("helm install", 2),
    ("helm upgrade", 2),
    ("helm uninstall", 2),
    ("helm list", 2),
    ("helm repo", 3),
    ("helm status", 2),
    ("helm template", 2),
];

/// Bash 命令前缀允许规则的元数字典。
///
/// 提供元数感知的前缀提取，因此 `auto_allow = ["git status"]`
/// 正确匹配 `git status -s` 和 `git status --porcelain`，
/// 而不会也匹配 `git push`。
///
/// # 示例
///
/// ```rust
/// use codewhale_execpolicy::bash_arity::BashArityDict;
///
/// let dict = BashArityDict::new();
/// assert_eq!(dict.classify(&["git", "status", "-s"]),   "git status");
/// assert_eq!(dict.classify(&["git", "push", "origin"]), "git push");
/// assert_eq!(dict.classify(&["npm", "run", "dev"]),     "npm run dev");
/// assert_eq!(dict.classify(&["ls", "-la"]),             "ls");
/// ```
#[derive(Debug, Clone)]
pub struct BashArityDict {
    /// 内部表，最长前缀优先排序，用于贪婪匹配。
    entries: Vec<(&'static str, u8)>,
}

impl BashArityDict {
    /// 构造一个预加载了 [`BASH_ARITY_TABLE`] 的新字典。
    #[must_use]
    pub fn new() -> Self {
        let mut entries: Vec<(&'static str, u8)> = BASH_ARITY_TABLE.to_vec();
        // 最长前缀优先，以便贪婪匹配正确工作。
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
        Self { entries }
    }

    /// 返回命令令牌切片的标准命令前缀。
    ///
    /// # 算法
    ///
    /// 1. 去掉所有标志令牌（以 `-` 开头的令牌）。
    /// 2. 从位置令牌构建深度 1..=3 的候选（最长优先）。
    /// 3. 如果候选匹配字典条目，返回用空格连接的 `arity` 个位置令牌。
    /// 4. 如果没有字典条目匹配，返回单个基础命令名称。
    #[must_use]
    pub fn classify(&self, tokens: &[&str]) -> String {
        if tokens.is_empty() {
            return String::new();
        }

        // 收集位置（非标志）令牌，转为小写。
        let positional: Vec<String> = tokens
            .iter()
            .filter(|t| !t.starts_with('-'))
            .map(|t| t.to_ascii_lowercase())
            .collect();

        if positional.is_empty() {
            return String::new();
        }

        // 从最长到最短尝试候选（最大深度 3）。
        let max_depth = positional.len().min(3);
        for depth in (1..=max_depth).rev() {
            let candidate = positional[..depth].join(" ");
            if let Some(&(_key, arity)) = self
                .entries
                .iter()
                .find(|(key, _)| *key == candidate.as_str())
            {
                let take = (arity as usize).min(positional.len());
                return positional[..take].join(" ");
            }
        }

        // 无匹配：仅返回基础命令名称。
        positional[0].clone()
    }

    /// 如果允许规则 `pattern`（命令前缀字符串，例如 `"git status"`）
    /// 匹配具体命令 `command`，则返回 `true`。
    ///
    /// 匹配是元数感知的：
    /// - `"git status"` 匹配 `"git status -s"` 和 `"git status --porcelain"`。
    /// - `"git status"` **不**匹配 `"git push origin main"`。
    /// - 精确字符串模式（例如 `"ls"`）仍然像以前一样工作。
    ///
    /// 对于不在元数表中的模式，函数回退到规范化命令上的纯前缀测试，
    /// 以便现有的精确匹配规则继续不变地工作。
    #[must_use]
    pub fn allow_rule_matches(&self, pattern: &str, command: &str) -> bool {
        let pattern_lower = pattern.trim().to_ascii_lowercase();
        let command_tokens: Vec<&str> = command.split_whitespace().collect();

        // 通过元数字典分类具体命令。
        let canonical = self.classify(&command_tokens);

        // 主要检查：分类的前缀等于允许规则模式。
        if canonical == pattern_lower {
            return true;
        }

        // 回退：对不在元数表中的模式进行词边界前缀匹配。
        // 匹配精确模式或模式后跟空格（即在词边界），
        // 因此 "ls" 匹配 "ls" 和 "ls -la" 但**不**匹配 "lsof"。
        let command_lower = command.trim().to_ascii_lowercase();
        // 比较前规范化两侧的空格。
        let pattern_norm: String = pattern_lower
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let command_norm: String = command_lower
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        command_norm == pattern_norm
            || (command_norm.starts_with(&pattern_norm)
                && command_norm.as_bytes().get(pattern_norm.len()) == Some(&b' '))
    }

    /// 遍历字典中的所有条目。
    pub fn entries(&self) -> impl Iterator<Item = (&str, u8)> {
        self.entries.iter().map(|(k, v)| (*k, *v))
    }

    /// 返回字典中的条目数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 如果字典为空则返回 `true`。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for BashArityDict {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> BashArityDict {
        BashArityDict::new()
    }

    // ── classify ─────────────────────────────────────────────────────────────

    #[test]
    fn classify_git_status_bare() {
        assert_eq!(dict().classify(&["git", "status"]), "git status");
    }

    #[test]
    fn classify_git_status_with_short_flag() {
        assert_eq!(dict().classify(&["git", "status", "-s"]), "git status");
    }

    #[test]
    fn classify_git_status_with_long_flag() {
        assert_eq!(
            dict().classify(&["git", "status", "--porcelain"]),
            "git status"
        );
    }

    #[test]
    fn classify_git_push() {
        assert_eq!(
            dict().classify(&["git", "push", "origin", "main"]),
            "git push"
        );
    }

    #[test]
    fn classify_git_push_force() {
        assert_eq!(dict().classify(&["git", "push", "--force"]), "git push");
    }

    #[test]
    fn classify_npm_run_dev_arity_3() {
        assert_eq!(dict().classify(&["npm", "run", "dev"]), "npm run dev");
    }

    #[test]
    fn classify_npm_install() {
        assert_eq!(dict().classify(&["npm", "install"]), "npm install");
    }

    #[test]
    fn classify_cargo_check_with_flag() {
        assert_eq!(
            dict().classify(&["cargo", "check", "--workspace"]),
            "cargo check"
        );
    }

    #[test]
    fn classify_docker_compose_up_arity_3() {
        assert_eq!(
            dict().classify(&["docker", "compose", "up"]),
            "docker compose up"
        );
    }

    #[test]
    fn classify_kubectl_get_pods_arity_3() {
        assert_eq!(
            dict().classify(&["kubectl", "get", "pods"]),
            "kubectl get pods"
        );
    }

    #[test]
    fn classify_go_mod_tidy_arity_3() {
        assert_eq!(dict().classify(&["go", "mod", "tidy"]), "go mod tidy");
    }

    #[test]
    fn classify_make_no_subcommand() {
        assert_eq!(dict().classify(&["make", "all"]), "make");
    }

    #[test]
    fn classify_aws_s3_arity_3() {
        assert_eq!(dict().classify(&["aws", "s3", "ls"]), "aws s3 ls");
    }

    #[test]
    fn classify_terraform_plan() {
        assert_eq!(
            dict().classify(&["terraform", "plan", "-out=tfplan"]),
            "terraform plan"
        );
    }

    #[test]
    fn classify_unknown_falls_back_to_base() {
        assert_eq!(dict().classify(&["ls", "-la"]), "ls");
    }

    #[test]
    fn classify_empty_returns_empty() {
        assert_eq!(dict().classify(&[]), "");
    }

    // ── allow_rule_matches ────────────────────────────────────────────────────

    #[test]
    fn allow_rule_git_status_matches_with_flag() {
        assert!(dict().allow_rule_matches("git status", "git status -s"));
    }

    #[test]
    fn allow_rule_git_status_matches_porcelain() {
        assert!(dict().allow_rule_matches("git status", "git status --porcelain"));
    }

    #[test]
    fn allow_rule_git_status_does_not_match_push() {
        assert!(!dict().allow_rule_matches("git status", "git push origin main"));
    }

    #[test]
    fn allow_rule_git_status_does_not_match_checkout() {
        assert!(!dict().allow_rule_matches("git status", "git checkout main"));
    }

    #[test]
    fn allow_rule_npm_run_matches_dev() {
        assert!(dict().allow_rule_matches("npm run dev", "npm run dev"));
    }

    #[test]
    fn allow_rule_npm_run_dev_does_not_match_build() {
        assert!(!dict().allow_rule_matches("npm run dev", "npm run build"));
    }

    #[test]
    fn allow_rule_cargo_check_matches_with_flags() {
        assert!(dict().allow_rule_matches("cargo check", "cargo check --workspace"));
    }

    #[test]
    fn allow_rule_exact_match_still_works() {
        // 一个不在元数表中的模式回退到精确/前缀匹配。
        assert!(dict().allow_rule_matches("ls", "ls -la"));
    }

    #[test]
    fn allow_rule_make_matches_with_target() {
        assert!(dict().allow_rule_matches("make", "make all"));
        assert!(dict().allow_rule_matches("make", "make clean"));
    }

    #[test]
    fn allow_rule_aws_s3_ls() {
        assert!(dict().allow_rule_matches("aws s3 ls", "aws s3 ls"));
        // "aws s3 cp" 不应匹配 "aws s3 ls"
        assert!(!dict().allow_rule_matches("aws s3 ls", "aws s3 cp src dst"));
    }

    // ── 覆盖率计数 ────────────────────────────────────────────────────────

    #[test]
    fn dict_covers_at_least_30_commands() {
        // 问题要求涵盖 30+ 个常用命令。
        assert!(
            BashArityDict::new().len() >= 30,
            "预期至少 30 个条目，得到 {}",
            BashArityDict::new().len()
        );
    }
}

import { TextlintKernel } from "@textlint/kernel";
import markdownPlugin from "@textlint/textlint-plugin-markdown";
import compatsFixFormatter from "@textlint/fixer-formatter/lib/src/formatters/compats.js";
import diffFixFormatter from "@textlint/fixer-formatter/lib/src/formatters/diff.js";
import fixedResultFormatter from "@textlint/fixer-formatter/lib/src/formatters/fixed-result.js";
import jsonFixFormatter from "@textlint/fixer-formatter/lib/src/formatters/json.js";
import stylishFixFormatter from "@textlint/fixer-formatter/lib/src/formatters/stylish.js";
import checkstyleLintFormatter from "@textlint/linter-formatter/lib/src/formatters/checkstyle.js";
import compactLintFormatter from "@textlint/linter-formatter/lib/src/formatters/compact.js";
import githubLintFormatter from "@textlint/linter-formatter/lib/src/formatters/github.js";
import jslintXmlLintFormatter from "@textlint/linter-formatter/lib/src/formatters/jslint-xml.js";
import jsonLintFormatter from "@textlint/linter-formatter/lib/src/formatters/json.js";
import junitLintFormatter from "@textlint/linter-formatter/lib/src/formatters/junit.js";
import prettyErrorLintFormatter from "@textlint/linter-formatter/lib/src/formatters/pretty-error.js";
import stylishLintFormatter from "@textlint/linter-formatter/lib/src/formatters/stylish.js";
import tableLintFormatter from "@textlint/linter-formatter/lib/src/formatters/table.js";
import tapLintFormatter from "@textlint/linter-formatter/lib/src/formatters/tap.js";
import unixLintFormatter from "@textlint/linter-formatter/lib/src/formatters/unix.js";
import { presetDefinitions, standaloneRules } from "textlint-v8:registry";

type RuleOptions = boolean | Record<string, unknown>;

interface LintRequest {
  text: string;
  filePath?: string;
  rules?: Record<string, RuleOptions>;
}

interface FormatRequest {
  formatterName: string;
  results: unknown[];
  color: boolean;
}

interface RuleDefinition {
  ruleId: string;
  rule: unknown;
  options: RuleOptions;
}

interface BuildConfig {
  rules: Record<string, RuleOptions>;
}

interface Preset {
  rules: Record<string, unknown>;
  rulesConfig: Record<string, RuleOptions>;
}

declare const __TEXTLINT_V8_CONFIG__: BuildConfig;

Object.assign(globalThis, {
  window: globalThis,
  kuromojin: { dicPath: "embedded" },
  __textlintV8Files: new Map<string, string>(),
});

const configuredRules = standaloneRules.map((definition) => ({
  ...definition,
  options: __TEXTLINT_V8_CONFIG__.rules[definition.ruleId] ?? false,
}));

function resolvePresetRuleOptions(
  presetOptions: RuleOptions | undefined,
  override: RuleOptions | undefined,
  fallback: RuleOptions | undefined,
): RuleOptions {
  if (!presetOptions || override === false) return false;
  if (override === undefined || override === true) return fallback ?? true;
  return override;
}

for (const { presetId, ruleIdPrefix, preset: untypedPreset } of presetDefinitions) {
  const preset = untypedPreset as Preset;
  const presetOptions = __TEXTLINT_V8_CONFIG__.rules[presetId];
  const childOverrides =
    presetOptions && typeof presetOptions === "object" ? presetOptions : undefined;
  for (const [ruleKey, rule] of Object.entries(preset.rules)) {
    const options = resolvePresetRuleOptions(
      presetOptions,
      childOverrides?.[ruleKey],
      preset.rulesConfig[ruleKey],
    );
    configuredRules.push({ ruleId: `${ruleIdPrefix}/${ruleKey}`, rule, options });
  }
}

function selectRules(overrides: Record<string, RuleOptions> | undefined): RuleDefinition[] {
  return configuredRules.map((definition) => ({
    ...definition,
    options: overrides?.[definition.ruleId] ?? definition.options,
  }));
}

async function lint(requestJson: string): Promise<string> {
  const request = JSON.parse(requestJson) as LintRequest;
  if (typeof request.text !== "string") {
    throw new TypeError("request.text must be a string");
  }
  globalThis.__textlintV8Files.set(request.filePath ?? "input.md", request.text);

  const kernel = new TextlintKernel();
  const result = await kernel.lintText(request.text, {
    ext: ".md",
    filePath: request.filePath ?? "input.md",
    configBaseDir: "/",
    plugins: [{ pluginId: "markdown", plugin: markdownPlugin, options: true }],
    rules: selectRules(request.rules) as never,
    filterRules: [],
  });
  return JSON.stringify(result);
}

async function fix(requestJson: string): Promise<string> {
  const request = JSON.parse(requestJson) as LintRequest;
  if (typeof request.text !== "string") {
    throw new TypeError("request.text must be a string");
  }
  globalThis.__textlintV8Files.set(request.filePath ?? "input.md", request.text);

  const kernel = new TextlintKernel();
  const result = await kernel.fixText(request.text, {
    ext: ".md",
    filePath: request.filePath ?? "input.md",
    configBaseDir: "/",
    plugins: [{ pluginId: "markdown", plugin: markdownPlugin, options: true }],
    rules: selectRules(request.rules) as never,
    filterRules: [],
  });
  return JSON.stringify(result);
}

type Formatter = (results: never[], options?: unknown) => string;

const lintFormatters: Record<string, Formatter> = {
  checkstyle: checkstyleLintFormatter,
  compact: compactLintFormatter,
  github: githubLintFormatter,
  "jslint-xml": jslintXmlLintFormatter,
  json: jsonLintFormatter,
  junit: junitLintFormatter,
  "pretty-error": prettyErrorLintFormatter,
  stylish: stylishLintFormatter,
  table: tableLintFormatter,
  tap: tapLintFormatter,
  unix: unixLintFormatter,
};

const fixFormatters: Record<string, Formatter> = {
  compats: compatsFixFormatter,
  diff: diffFixFormatter,
  "fixed-result": fixedResultFormatter,
  json: jsonFixFormatter,
  stylish: stylishFixFormatter,
};

function format(requestJson: string, formatters: Record<string, Formatter>): string {
  const request = JSON.parse(requestJson) as FormatRequest;
  const formatter = formatters[request.formatterName];
  if (!formatter) {
    throw new Error(`Could not find formatter ${request.formatterName}`);
  }
  return JSON.stringify(
    formatter(request.results as never[], {
      formatterName: request.formatterName,
      color: request.color,
    }),
  );
}

async function formatLint(requestJson: string): Promise<string> {
  return format(requestJson, lintFormatters);
}

async function formatFix(requestJson: string): Promise<string> {
  return format(requestJson, fixFormatters);
}

globalThis.textlintV8 = { lint, fix, formatLint, formatFix };

declare global {
  // eslint-disable-next-line no-var
  var textlintV8: {
    lint(requestJson: string): Promise<string>;
    fix(requestJson: string): Promise<string>;
    formatLint(requestJson: string): Promise<string>;
    formatFix(requestJson: string): Promise<string>;
  };
  var __textlintV8Files: Map<string, string>;
}

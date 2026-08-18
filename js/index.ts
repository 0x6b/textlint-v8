import { TextlintKernel } from "@textlint/kernel";
import markdownPlugin from "@textlint/textlint-plugin-markdown";
import checkstyleFormatter from "@textlint/linter-formatter/lib/src/formatters/checkstyle.js";
import compactFormatter from "@textlint/linter-formatter/lib/src/formatters/compact.js";
import jsonFormatter from "@textlint/linter-formatter/lib/src/formatters/json.js";
import junitFormatter from "@textlint/linter-formatter/lib/src/formatters/junit.js";
import stylishFormatter from "@textlint/linter-formatter/lib/src/formatters/stylish.js";
import tapFormatter from "@textlint/linter-formatter/lib/src/formatters/tap.js";
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

const formatters: Record<string, (results: never[], options?: unknown) => string> = {
  checkstyle: checkstyleFormatter,
  compact: compactFormatter,
  json: jsonFormatter,
  junit: junitFormatter,
  stylish: stylishFormatter,
  tap: tapFormatter,
};

async function format(requestJson: string): Promise<string> {
  const request = JSON.parse(requestJson) as FormatRequest;
  const formatter = formatters[request.formatterName];
  if (!formatter) {
    throw new Error(`Could not find formatter ${request.formatterName}`);
  }
  return JSON.stringify(
    formatter(request.results as never[], { formatterName: request.formatterName }),
  );
}

globalThis.textlintV8 = { lint, fix, format };

declare global {
  // eslint-disable-next-line no-var
  var textlintV8: {
    lint(requestJson: string): Promise<string>;
    fix(requestJson: string): Promise<string>;
    format(requestJson: string): Promise<string>;
  };
}

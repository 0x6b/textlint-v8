import { TextlintKernel } from "@textlint/kernel";
import markdownPlugin from "@textlint/textlint-plugin-markdown";
import { presetDefinitions, standaloneRules } from "textlint-v8:registry";

type RuleOptions = boolean | Record<string, unknown>;

interface LintRequest {
  text: string;
  filePath?: string;
  rules?: Record<string, RuleOptions>;
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

for (const { presetId, ruleIdPrefix, preset: untypedPreset } of presetDefinitions) {
  const preset = untypedPreset as Preset;
  const presetOptions = __TEXTLINT_V8_CONFIG__.rules[presetId];
  const childOverrides =
    presetOptions && typeof presetOptions === "object" ? presetOptions : undefined;
  for (const [ruleKey, rule] of Object.entries(preset.rules)) {
    const override = childOverrides?.[ruleKey];
    const options =
      presetOptions === false || presetOptions === undefined
        ? false
        : override === false
          ? false
          : override !== undefined && override !== true
            ? override
            : (preset.rulesConfig[ruleKey] ?? true);
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

globalThis.textlintV8 = { lint };

declare global {
  // eslint-disable-next-line no-var
  var textlintV8: { lint(requestJson: string): Promise<string> };
}

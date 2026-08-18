import { TextlintKernel } from "@textlint/kernel";
import markdownPlugin from "@textlint/textlint-plugin-markdown";
import noEmoji from "@0x6b/textlint-rule-no-emoji";
import noEmphasis from "@0x6b/textlint-rule-no-emphasis";
import noHrBeforeHeading from "@0x6b/textlint-rule-no-hr-before-heading";
import noNumberedHeadingsAndBullets from "@0x6b/textlint-rule-no-numbered-headings-and-bullets";
import noSmartQuotes from "@0x6b/textlint-rule-no-smart-quotes";
import normalizeWhitespaces from "@0x6b/textlint-rule-normalize-whitespaces";
import aiWritingPreset from "@textlint-ja/textlint-rule-preset-ai-writing";
import technicalWritingPreset from "textlint-rule-preset-ja-technical-writing";

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

interface PresetDefinition {
  presetId: string;
  ruleIdPrefix: string;
  preset: Preset;
}

declare const __TEXTLINT_V8_CONFIG__: BuildConfig;

Object.assign(globalThis, {
  window: globalThis,
  kuromojin: { dicPath: "embedded" },
});

const standaloneRules: RuleDefinition[] = [
  { ruleId: "@0x6b/no-emoji", rule: noEmoji, options: true },
  { ruleId: "@0x6b/no-emphasis", rule: noEmphasis, options: true },
  { ruleId: "@0x6b/no-hr-before-heading", rule: noHrBeforeHeading, options: true },
  {
    ruleId: "@0x6b/no-numbered-headings-and-bullets",
    rule: noNumberedHeadingsAndBullets,
    options: true,
  },
  { ruleId: "@0x6b/no-smart-quotes", rule: noSmartQuotes, options: true },
  { ruleId: "@0x6b/normalize-whitespaces", rule: normalizeWhitespaces, options: true },
];

const presetDefinitions: PresetDefinition[] = [
  {
    presetId: "@textlint-ja/preset-ai-writing",
    ruleIdPrefix: "@textlint-ja/ai-writing",
    preset: aiWritingPreset as Preset,
  },
  {
    presetId: "preset-ja-technical-writing",
    ruleIdPrefix: "ja-technical-writing",
    preset: technicalWritingPreset as Preset,
  },
];

const availableRuleIds = new Set([
  ...standaloneRules.map(({ ruleId }) => ruleId),
  ...presetDefinitions.map(({ presetId }) => presetId),
]);
for (const ruleId of Object.keys(__TEXTLINT_V8_CONFIG__.rules)) {
  if (!availableRuleIds.has(ruleId)) {
    throw new Error(`Unknown bundled textlint rule: ${ruleId}`);
  }
}

const configuredRules = standaloneRules.map((definition) => ({
  ...definition,
  options: __TEXTLINT_V8_CONFIG__.rules[definition.ruleId] ?? false,
}));

for (const { presetId, ruleIdPrefix, preset } of presetDefinitions) {
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

import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import vm from "node:vm";
import { build } from "esbuild";

const configPath = path.resolve(process.env.TEXTLINT_V8_CONFIG ?? "textlint-v8.config.json");
const outfile = path.resolve("dist/textlint-v8.js");

let config;
try {
  config = JSON.parse(await readFile(configPath, "utf8"));
} catch (error) {
  throw new Error(`failed to read textlint config ${configPath}`, { cause: error });
}

if (!config || typeof config !== "object" || Array.isArray(config)) {
  throw new TypeError("textlint config must be an object");
}
if (!config.rules || typeof config.rules !== "object" || Array.isArray(config.rules)) {
  throw new TypeError("textlint config must contain a rules object");
}
for (const [ruleId, options] of Object.entries(config.rules)) {
  if (
    typeof options !== "boolean" &&
    (!options || typeof options !== "object" || Array.isArray(options))
  ) {
    throw new TypeError(`configuration for ${ruleId} must be a boolean or object`);
  }
}

const assertShim = path.resolve("js/assert-shim.cjs");
const pathShim = path.resolve("js/path-shim.ts");
const osShim = path.resolve("js/os-shim.ts");
const fsShim = path.resolve("js/fs-shim.ts");
const dictionaryLoader = path.resolve("js/embedded-dictionary-loader.cjs");
const conjunctiveParticleGaRule = path.resolve(
  "node_modules/textlint-rule-no-doubled-conjunctive-particle-ga/lib/no-doubled-conjunctive-particle-ga.js",
);

function replaceExactly(source, before, after) {
  if (!source.includes(before)) {
    throw new Error("textlint-rule-no-doubled-conjunctive-particle-ga changed; update cache patch");
  }
  return source.replace(before, after);
}

const result = await build({
  entryPoints: ["js/index.ts"],
  bundle: true,
  format: "iife",
  globalName: "__textlintV8Bundle",
  platform: "browser",
  target: "es2020",
  alias: {
    assert: assertShim,
    "node:assert": assertShim,
    path: pathShim,
    "node:path": pathShim,
    os: osShim,
    "node:os": osShim,
    fs: fsShim,
    "node:fs": fsShim,
  },
  define: {
    __TEXTLINT_V8_CONFIG__: JSON.stringify(config),
  },
  plugins: [
    {
      name: "embedded-kuromoji-dictionary",
      setup(build) {
        build.onResolve({ filter: /NodeDictionaryLoader(?:\.js)?$/ }, (args) => {
          if (args.importer.includes(`${path.sep}kuromoji${path.sep}`)) {
            return { path: dictionaryLoader };
          }
        });
      },
    },
    {
      name: "cache-conjunctive-particle-ga-tokenization",
      setup(build) {
        build.onLoad({ filter: /no-doubled-conjunctive-particle-ga\.js$/ }, async (args) => {
          if (args.path !== conjunctiveParticleGaRule) return;
          let source = await readFile(args.path, "utf8");
          source = replaceExactly(
            source,
            "return (0, _kuromojin.getTokenizer)().then(tokenizer => {\n        var checkSentence = sentence => {",
            "var checkSentence = async sentence => {",
          );
          source = replaceExactly(
            source,
            "var tokens = tokenizer.tokenizeForSentence(sentenceText);",
            "var tokens = await (0, _kuromojin.tokenize)(sentenceText);",
          );
          source = replaceExactly(
            source,
            "        sentences.forEach(checkSentence);\n      });",
            "        return Promise.all(sentences.map(checkSentence));",
          );
          return { contents: source, loader: "js" };
        });
      },
    },
  ],
  outfile,
  write: false,
});

// Evaluate the generated bundle during the build so unsupported rule names fail immediately.
const bundle = result.outputFiles[0].text;
globalThis.__loadKuromojiDictionary = () => new ArrayBuffer(0);
vm.runInThisContext(bundle, { filename: outfile });
delete globalThis.textlintV8;
delete globalThis.__loadKuromojiDictionary;

await mkdir(path.dirname(outfile), { recursive: true });
await writeFile(outfile, bundle);
console.log(`Embedded textlint config: ${configPath}`);

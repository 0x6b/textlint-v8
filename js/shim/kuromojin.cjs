const dictionaryPath = "embedded:kuromojin.dict";

exports.defaultDictionaryPath = () => dictionaryPath;
exports.getPlatform = () => ({
  async read(path) {
    if (path !== dictionaryPath) throw new Error(`Unknown dictionary: ${path}`);
    return new Uint8Array(__loadKuromojinDictionary());
  },
  async gunzip(data, rawLength) {
    return new Uint8Array(__gunzipKuromojinDictionaryPart(data, rawLength));
  },
});

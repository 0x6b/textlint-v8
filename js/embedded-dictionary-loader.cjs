const DictionaryLoader = require("kuromoji/src/loader/DictionaryLoader.js");

class EmbeddedDictionaryLoader extends DictionaryLoader {
  loadArrayBuffer(url, callback) {
    const filename = url.split("/").pop() || "";
    try {
      callback(null, __loadKuromojiDictionary(filename));
    } catch (error) {
      callback(error);
    }
  }
}

module.exports = EmbeddedDictionaryLoader;

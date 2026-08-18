const DictionaryLoader = require("kuromoji/src/loader/DictionaryLoader.js");

function EmbeddedDictionaryLoader(dictionaryPath) {
  DictionaryLoader.call(this, dictionaryPath);
}

EmbeddedDictionaryLoader.prototype = Object.create(DictionaryLoader.prototype);
EmbeddedDictionaryLoader.prototype.loadArrayBuffer = function loadArrayBuffer(url, callback) {
  const filename = url.split("/").pop() || "";
  try {
    callback(null, __loadKuromojiDictionary(filename));
  } catch (error) {
    callback(error, null);
  }
};

module.exports = EmbeddedDictionaryLoader;

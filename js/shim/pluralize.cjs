module.exports = function pluralize(word, count, inclusive) {
  var value = count === 1 ? word : word + "s";
  return inclusive ? count + " " + value : value;
};

// Prism themes for the Ailu docs: code sits on the sunken paper, in brown ink, with the state
// materials' text tones for syntax. Every token colour clears 4.5:1 on its background.

/** @type {import('prism-react-renderer').PrismTheme} */
const light = {
  plain: { color: "#2B2723", backgroundColor: "#F5EADA" },
  styles: [
    { types: ["comment", "prolog", "doctype", "cdata"], style: { color: "#6B5D48", fontStyle: "italic" } },
    { types: ["punctuation", "operator"], style: { color: "#4E4335" } },
    { types: ["keyword", "atrule", "selector", "important"], style: { color: "#A94F24" } },
    { types: ["string", "char", "attr-value", "inserted", "regex", "template-string"], style: { color: "#3F6130" } },
    { types: ["number", "boolean", "constant", "symbol"], style: { color: "#3B566E" } },
    { types: ["function", "class-name", "builtin", "maybe-class-name"], style: { color: "#61407A" } },
    { types: ["tag", "deleted"], style: { color: "#8E2F28" } },
    { types: ["property", "attr-name", "variable", "parameter"], style: { color: "#4E4335" } }
  ]
};

/** @type {import('prism-react-renderer').PrismTheme} */
const dark = {
  plain: { color: "#F3ECE2", backgroundColor: "#120F0D" },
  styles: [
    { types: ["comment", "prolog", "doctype", "cdata"], style: { color: "#A99A86", fontStyle: "italic" } },
    { types: ["punctuation", "operator"], style: { color: "#CFC3B2" } },
    { types: ["keyword", "atrule", "selector", "important"], style: { color: "#E08A5E" } },
    { types: ["string", "char", "attr-value", "inserted", "regex", "template-string"], style: { color: "#A6C98F" } },
    { types: ["number", "boolean", "constant", "symbol"], style: { color: "#A5C0D6" } },
    { types: ["function", "class-name", "builtin", "maybe-class-name"], style: { color: "#CDB0E0" } },
    { types: ["tag", "deleted"], style: { color: "#EE9A92" } },
    { types: ["property", "attr-name", "variable", "parameter"], style: { color: "#CFC3B2" } }
  ]
};

module.exports = { light, dark };

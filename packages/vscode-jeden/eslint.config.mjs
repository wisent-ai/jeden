import tseslint from "typescript-eslint";

// Shapes a defect was once filed against, refused where they would come back.
// A field an agent's message lacks is refused naming the message and the field
// (required() in src/acp/protocol.ts), never filled with a value nobody sent.
const RESTRICTED = [
  {
    selector: "LogicalExpression[operator=/^(\\?\\?|\\|\\|)$/][right.type='Literal'][right.value='']",
    message: "A missing text is not the empty string: refuse it naming the field (required() in src/acp/protocol.ts).",
  },
  {
    selector: "LogicalExpression[operator=/^(\\?\\?|\\|\\|)$/][right.type='Literal'][right.value=/^(default|unknown|answer)$/]",
    message: "A missing value is not 'default', 'unknown' or 'answer': refuse it naming the field.",
  },
  {
    selector: "LogicalExpression[operator=/^(\\?\\?|\\|\\|)$/][right.type='ObjectExpression'][right.properties.length=0]",
    message: "A missing object is not an empty one: refuse it naming the field.",
  },
  {
    // `...(present ? { field } : {})` adds an optional field only when it is there; any other `: {}` invents a missing object.
    selector: ":not(SpreadElement) > ConditionalExpression[test.type='CallExpression'][alternate.type='ObjectExpression'][alternate.properties.length=0]",
    message: "A missing object is not an empty one: refuse it naming the field, or spread the optional field only when present.",
  },
];

export default tseslint.config(
  { ignores: ["dist/**", "node_modules/**"] },
  {
    files: ["src/**/*.ts"],
    languageOptions: { parser: tseslint.parser },
    rules: { "no-restricted-syntax": ["error", ...RESTRICTED] },
  },
);

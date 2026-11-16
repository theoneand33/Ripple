import test from "node:test";
import assert from "node:assert/strict";
import { tokenizeArgs, parseCommand } from "./command.mjs";

test("tokenizeArgs keeps mid-token quotes together", () => {
  assert.deepEqual(tokenizeArgs('--flag="hello world" "quoted arg" --bare'), ["--flag=hello world", "quoted arg", "--bare"]);
  assert.deepEqual(tokenizeArgs("   "), []);
});

test("parseCommand splits exe and args", () => {
  assert.deepEqual(parseCommand('"C:\\Program Files\\x.exe" --a="b c"'), { exe: "C:\\Program Files\\x.exe", args: ["--a=b c"] });
  assert.deepEqual(parseCommand("C:/Program Files/App/app.exe -v"), { exe: "C:\\Program Files\\App\\app.exe", args: ["-v"] });
  assert.deepEqual(parseCommand("wt new-tab"), { exe: "wt", args: ["new-tab"] });
  assert.deepEqual(parseCommand("cmd"), { exe: "cmd", args: [] });
});

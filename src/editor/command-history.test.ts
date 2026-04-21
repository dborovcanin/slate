import test from "node:test";
import assert from "node:assert/strict";
import {
  CommandHistoryNavigator,
  getCommandHistoryForTests,
  rememberCommand,
  resetCommandHistoryForTests,
} from "./command-history.ts";

test("command history keeps latest unique commands and trims colon", () => {
  resetCommandHistoryForTests();

  rememberCommand(":sum");
  rememberCommand("format");
  rememberCommand("sum");

  assert.deepEqual(getCommandHistoryForTests(), ["format", "sum"]);
});

test("history navigator cycles with up/down semantics", () => {
  resetCommandHistoryForTests();
  rememberCommand("one");
  rememberCommand("two");
  rememberCommand("three");

  const nav = new CommandHistoryNavigator();
  assert.equal(nav.previous(), "three");
  assert.equal(nav.previous(), "two");
  assert.equal(nav.previous(), "one");
  assert.equal(nav.previous(), "three");

  assert.equal(nav.next(), "one");
  assert.equal(nav.next(), "two");
  assert.equal(nav.next(), "three");
  assert.equal(nav.next(), "one");
});

test("navigator reset restarts from latest command", () => {
  resetCommandHistoryForTests();
  rememberCommand("alpha");
  rememberCommand("beta");

  const nav = new CommandHistoryNavigator();
  assert.equal(nav.previous(), "beta");
  assert.equal(nav.previous(), "alpha");
  nav.reset();
  assert.equal(nav.previous(), "beta");
});

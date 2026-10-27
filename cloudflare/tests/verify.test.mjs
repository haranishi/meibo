import assert from "node:assert/strict";
import test from "node:test";
import { verifyService } from "../scripts/verify.mjs";

for (const profile of ["local", "public"]) test(`synthetic ${profile} verification refuses a foreign destination before browser startup`, async () => {
  await assert.rejects(verifyService({ origin: "https://untrusted.invalid", profile,
    credentials: { scope: "synthetic fixture only", password: "synthetic-password" }, output: "/not-accessed" }),
  (error) => error.code === "ERR_ASSERTION");
});
test("browser verification requires explicit synthetic credentials", async () => {
  await assert.rejects(verifyService({ origin: "https://localhost:4317", profile: "local",
    credentials: { scope: "ordinary-service", password: "synthetic-password" }, output: "/not-accessed" }),
  (error) => error.code === "ERR_ASSERTION");
});

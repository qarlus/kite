import { test, expect } from "./mock-ipc";

test("inbox renders threads from the backend", async ({ page }) => {
  await page.goto("/");
  await expect(page.getByRole("option").first()).toContainText("Subject 1");
});

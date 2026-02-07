import { test, expect } from "@playwright/test";

// A minimal valid SDP offer that str0m can parse.
// This is used for HTTP-level signaling tests.
const MINIMAL_SDP_OFFER = [
  "v=0",
  "o=- 0 0 IN IP4 127.0.0.1",
  "s=-",
  "t=0 0",
  "a=group:BUNDLE 0",
  "a=ice-options:trickle",
  "m=audio 9 UDP/TLS/RTP/SAVPF 111",
  "c=IN IP4 0.0.0.0",
  "a=mid:0",
  "a=sendrecv",
  "a=rtpmap:111 opus/48000/2",
  "a=ice-ufrag:testufrag1234",
  "a=ice-pwd:testpasswordtestpassword1234",
  "a=fingerprint:sha-256 00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00:00",
  "a=setup:actpass",
  "",
].join("\r\n");

test.describe("WHIP/WHEP HTTP Signaling", () => {
  test("OPTIONS /whip returns CORS headers and Accept-Patch", async ({
    request,
  }) => {
    const resp = await request.fetch("/whip", { method: "OPTIONS" });
    expect(resp.status()).toBe(204);
    expect(resp.headers()["access-control-allow-origin"]).toBe("*");
    expect(resp.headers()["access-control-allow-methods"]).toContain("POST");
    expect(resp.headers()["accept-patch"]).toBe(
      "application/trickle-ice-sdpfrag"
    );
  });

  test("OPTIONS /whip includes STUN Link header", async ({ request }) => {
    const resp = await request.fetch("/whip", { method: "OPTIONS" });
    const link = resp.headers()["link"];
    expect(link).toBeTruthy();
    expect(link).toContain("stun:");
    expect(link).toContain('rel="ice-server"');
  });

  test("POST /whip rejects non-SDP content type", async ({ request }) => {
    const resp = await request.post("/whip", {
      headers: { "Content-Type": "application/json" },
      data: "{}",
    });
    expect(resp.status()).toBe(415);
  });

  test("POST /whip rejects invalid SDP", async ({ request }) => {
    const resp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: "this is not valid SDP",
    });
    expect(resp.status()).toBe(400);
  });

  test("POST /whip with valid SDP returns 201 + Location + ETag", async ({
    request,
  }) => {
    const resp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(resp.status()).toBe(201);

    // Check response headers
    const location = resp.headers()["location"];
    expect(location).toBeTruthy();
    expect(location).toMatch(/^\/resource\//);

    const etag = resp.headers()["etag"];
    expect(etag).toBeTruthy();

    const contentType = resp.headers()["content-type"];
    expect(contentType).toContain("application/sdp");

    // Check SDP answer body
    const body = await resp.text();
    expect(body).toContain("v=0");
    expect(body).toContain("a=ice-ufrag:");

    // CORS headers
    expect(resp.headers()["access-control-allow-origin"]).toBe("*");

    // Clean up
    await request.delete(location!);
  });

  test("POST /whep with nonexistent publisher returns 404", async ({
    request,
  }) => {
    const resp = await request.post(
      "/whep/00000000-0000-0000-0000-000000000000",
      {
        headers: { "Content-Type": "application/sdp" },
        data: MINIMAL_SDP_OFFER,
      }
    );
    expect(resp.status()).toBe(404);
  });

  test("POST /whep with invalid publisher ID returns 400", async ({
    request,
  }) => {
    const resp = await request.post("/whep/not-a-uuid", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(resp.status()).toBe(400);
  });

  test("Full WHIP publish → WHEP subscribe → DELETE lifecycle", async ({
    request,
  }) => {
    // 1. WHIP Publish
    const whipResp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whipResp.status()).toBe(201);

    const publisherLocation = whipResp.headers()["location"]!;
    const publisherId = publisherLocation.replace("/resource/", "");
    expect(publisherId).toBeTruthy();

    // 2. WHEP Subscribe
    const whepResp = await request.post(`/whep/${publisherId}`, {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whepResp.status()).toBe(201);

    const subscriberLocation = whepResp.headers()["location"]!;
    expect(subscriberLocation).toMatch(/^\/resource\//);

    const whepBody = await whepResp.text();
    expect(whepBody).toContain("v=0");

    // 3. DELETE subscriber
    const delSubResp = await request.delete(subscriberLocation);
    expect(delSubResp.status()).toBe(200);

    // 4. DELETE publisher
    const delPubResp = await request.delete(publisherLocation);
    expect(delPubResp.status()).toBe(200);

    // 5. Verify both are gone
    const gone1 = await request.delete(publisherLocation);
    expect(gone1.status()).toBe(404);
    const gone2 = await request.delete(subscriberLocation);
    expect(gone2.status()).toBe(404);
  });

  test("DELETE /resource with nonexistent ID returns 404", async ({
    request,
  }) => {
    const resp = await request.delete(
      "/resource/00000000-0000-0000-0000-000000000000"
    );
    expect(resp.status()).toBe(404);
  });

  test("Deleting WHIP publisher cascades to WHEP subscribers", async ({
    request,
  }) => {
    // Publish
    const whipResp = await request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whipResp.status()).toBe(201);
    const pubLocation = whipResp.headers()["location"]!;
    const pubId = pubLocation.replace("/resource/", "");

    // Subscribe
    const whepResp = await request.post(`/whep/${pubId}`, {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whepResp.status()).toBe(201);
    const subLocation = whepResp.headers()["location"]!;

    // Delete publisher — should cascade to subscriber
    const delResp = await request.delete(pubLocation);
    expect(delResp.status()).toBe(200);

    // Subscriber should be gone too
    const subGone = await request.delete(subLocation);
    expect(subGone.status()).toBe(404);
  });

  test("Janus API coexists with WHIP/WHEP", async ({ request }) => {
    // WHIP endpoints work
    const whipOpts = await request.fetch("/whip", { method: "OPTIONS" });
    expect(whipOpts.status()).toBe(204);

    // Janus API still works
    const info = await request.get("/janus/info");
    expect(info.ok()).toBeTruthy();
    const json = await info.json();
    expect(json.janus).toBe("server_info");
  });
});

test.describe("WHIP/WHEP Browser Demo", () => {
  test("Demo page loads and has correct elements", async ({ page }) => {
    await page.goto("/rust-demos/whip-whep.html");

    await expect(page.locator("h1")).toHaveText("WHIP/WHEP Demo");
    await expect(page.locator("#btn-publish")).toBeVisible();
    await expect(page.locator("#btn-subscribe")).toBeDisabled();
    await expect(page.locator("#btn-stop")).toBeDisabled();
    await expect(page.locator("#ball-canvas")).toBeVisible();
  });

  test("WHEP Player page loads and has correct elements", async ({ page }) => {
    await page.goto("/rust-demos/whep-player.html");

    await expect(page.locator("h1")).toHaveText("WHEP Player");
    await expect(page.locator("#publisher-id")).toBeVisible();
    await expect(page.locator("#btn-watch")).toBeVisible();
    await expect(page.locator("#btn-stop")).toBeDisabled();
  });

  test("Index page loads and links all demos", async ({ page }) => {
    await page.goto("/rust-demos/index.html");

    await expect(page.locator("h1")).toContainText("Janus Gateway");
    // Check all demo cards are present
    await expect(page.locator('a[href="bouncingball.html"]')).toBeVisible();
    await expect(
      page.locator('a[href="streaming-viewer.html"]')
    ).toBeVisible();
    await expect(page.locator('a[href="whip-whep.html"]')).toBeVisible();
    await expect(page.locator('a[href="whep-player.html"]')).toBeVisible();
  });

  test("WHIP publish via demo page exchanges SDP successfully", async ({
    page,
  }) => {
    await page.goto("/rust-demos/whip-whep.html");

    // Click Publish
    await page.click("#btn-publish");

    // Wait for publisher info to appear (SDP exchange completed)
    await expect(page.locator("#publisher-info code")).toBeVisible({
      timeout: 15000,
    });

    // Verify publisher ID is a UUID
    const publisherId = await page.locator("#publisher-info code").innerText();
    expect(publisherId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
    );

    // Subscribe button should be enabled now
    await expect(page.locator("#btn-subscribe")).toBeEnabled();
    await expect(page.locator("#btn-stop")).toBeEnabled();

    // Check status log for success messages
    const statusText = await page.locator("#status").innerText();
    expect(statusText).toContain("WHIP answer received");
    expect(statusText).toContain("WHIP publish active");

    // Stop
    await page.click("#btn-stop");
    await expect(page.locator("#btn-publish")).toBeEnabled({ timeout: 5000 });
  });

  test("WHIP publish then WHEP subscribe via demo page", async ({ page }) => {
    await page.goto("/rust-demos/whip-whep.html");

    // Publish
    await page.click("#btn-publish");
    await expect(page.locator("#publisher-info code")).toBeVisible({
      timeout: 15000,
    });

    // Subscribe
    await page.click("#btn-subscribe");

    // Wait for WHEP answer
    await page.waitForFunction(
      () => {
        const status = document.getElementById("status");
        return status && status.innerText.includes("WHEP subscribe active");
      },
      { timeout: 15000 }
    );

    // Verify subscribe success in status log
    const statusText = await page.locator("#status").innerText();
    expect(statusText).toContain("WHEP answer received");
    expect(statusText).toContain("WHEP subscribe active");

    // The remote video box should be visible (ontrack fires when ICE connects)
    // Note: ICE connectivity depends on network; we verify the signaling succeeded.
    // In headless/CI environments, ICE may not fully connect.

    // Stop
    await page.click("#btn-stop");
    await expect(page.locator("#btn-publish")).toBeEnabled({ timeout: 5000 });
  });

  test("WHEP Player subscribes to WHIP publisher", async ({ page }) => {
    // First, create a publisher via HTTP API
    const whipResp = await page.request.post("/whip", {
      headers: { "Content-Type": "application/sdp" },
      data: MINIMAL_SDP_OFFER,
    });
    expect(whipResp.status()).toBe(201);
    const pubLocation = whipResp.headers()["location"]!;
    const publisherId = pubLocation.replace("/resource/", "");

    // Navigate to WHEP player
    await page.goto("/rust-demos/whep-player.html");

    // Paste publisher ID
    await page.fill("#publisher-id", publisherId);

    // Click Watch
    await page.click("#btn-watch");

    // Wait for WHEP exchange to complete
    await page.waitForFunction(
      () => {
        const status = document.getElementById("status");
        return (
          status &&
          (status.innerText.includes("WHEP subscribe active") ||
            status.innerText.includes("WHEP error"))
        );
      },
      { timeout: 15000 }
    );

    const statusText = await page.locator("#status").innerText();
    expect(statusText).toContain("WHEP answer received");
    expect(statusText).toContain("WHEP subscribe active");

    // Stop
    await page.click("#btn-stop");

    // Clean up publisher
    await page.request.delete(pubLocation);
  });

  test("WHEP Player shows error for invalid publisher", async ({ page }) => {
    await page.goto("/rust-demos/whep-player.html");
    await page.fill(
      "#publisher-id",
      "00000000-0000-0000-0000-000000000000"
    );
    await page.click("#btn-watch");

    // Should show error in status
    await page.waitForFunction(
      () => {
        const status = document.getElementById("status");
        return status && status.innerText.includes("WHEP error");
      },
      { timeout: 10000 }
    );

    const statusText = await page.locator("#status").innerText();
    expect(statusText).toContain("404");
  });
});

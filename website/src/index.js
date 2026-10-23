// Serves the static site, sends www.agentscreen.app to the apex and plain http to https,
// and answers byte ranges for the films, which Safari needs to play MP4.
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const onWww = url.hostname === "www.agentscreen.app";
    // Cloudflare's edge reports the visitor's scheme here; wrangler dev doesn't.
    const plainHttp = (request.headers.get("cf-visitor") || "").includes('"http"');
    if (onWww || plainHttp) {
      url.hostname = "agentscreen.app";
      url.protocol = "https:";
      return Response.redirect(url.toString(), 301);
    }
    const range = request.headers.get("range");
    if (!url.pathname.startsWith("/media/")) return env.ASSETS.fetch(request);
    // Ask for the whole file, but keep the revalidation headers so a repeat visit gets a 304.
    const ask = new Headers(request.headers);
    ask.delete("range");
    const full = await env.ASSETS.fetch(new Request(request.url, { method: request.method, headers: ask }));
    if (full.status !== 200) return full;
    const headers = new Headers(full.headers);
    headers.set("accept-ranges", "bytes");
    if (!range || request.method === "HEAD") return new Response(full.body, { status: 200, headers });
    return byteRange(await full.arrayBuffer(), range, headers);
  },
};

// One range of the form bytes=a-b, a- or -n; anything else gets the whole file.
function byteRange(body, range, headers) {
  const size = body.byteLength;
  const m = /^bytes=(\d*)-(\d*)$/.exec(range.trim());
  if (!m || (m[1] === "" && m[2] === "")) return new Response(body, { status: 200, headers });
  let start, end;
  if (m[1] === "") {
    start = Math.max(0, size - Number(m[2]));
    end = size - 1;
  } else {
    start = Number(m[1]);
    end = m[2] === "" ? size - 1 : Math.min(Number(m[2]), size - 1);
  }
  if (start >= size || start > end) {
    headers.set("content-range", `bytes */${size}`);
    return new Response(null, { status: 416, headers });
  }
  headers.set("content-range", `bytes ${start}-${end}/${size}`);
  headers.set("content-length", String(end - start + 1));
  return new Response(body.slice(start, end + 1), { status: 206, headers });
}

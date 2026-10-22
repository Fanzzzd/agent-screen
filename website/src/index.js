// Serves the static site, sends www.agentscreen.app to the apex and plain http to https.
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
    return env.ASSETS.fetch(request);
  },
};

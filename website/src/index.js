// Serves the static site, sends www.agentscreen.app to the apex and plain http to https.
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const onWww = url.hostname === "www.agentscreen.app";
    const onApex = url.hostname === "agentscreen.app";
    if (onWww || (onApex && url.protocol === "http:")) {
      url.hostname = "agentscreen.app";
      url.protocol = "https:";
      return Response.redirect(url.toString(), 301);
    }
    return env.ASSETS.fetch(request);
  },
};

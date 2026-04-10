// Cloudflare Pages Function: inject dynamic OG tags for room code URLs.
// Matches /<CODE> paths (4-8 uppercase alphanumeric).

export async function onRequest(context) {
  const url = new URL(context.request.url);
  const path = url.pathname.slice(1); // remove leading /

  // Only intercept room code paths (4-8 chars, uppercase + digits).
  const isRoomCode = /^[A-Z0-9]{4,8}$/.test(path);

  if (!isRoomCode) {
    return context.next();
  }

  // Check if request is from a bot/crawler (link preview).
  const ua = (context.request.headers.get("user-agent") || "").toLowerCase();
  const isBot = /bot|crawler|spider|preview|facebookexternalhit|twitterbot|slackbot|telegram|whatsapp|imessagebot|applebot|linkedinbot|discord/i.test(ua);

  if (!isBot) {
    return context.next();
  }

  // Fetch original page.
  const response = await context.env.ASSETS.fetch(context.request);
  let html = await response.text();

  // Inject OG tags before </head>.
  const ogTags = `
    <meta property="og:title" content="txxxt — join call ${path}" />
    <meta property="og:description" content="Terminal video chat. Room code: ${path}" />
    <meta property="og:type" content="website" />
    <meta property="og:url" content="https://txxxt.me/${path}" />
    <meta name="twitter:card" content="summary" />
    <meta name="twitter:title" content="txxxt — join call ${path}" />
    <meta name="twitter:description" content="Terminal video chat. Room code: ${path}" />
  `;

  html = html.replace("</head>", ogTags + "</head>");

  return new Response(html, {
    headers: {
      ...Object.fromEntries(response.headers),
      "content-type": "text/html;charset=UTF-8",
    },
  });
}

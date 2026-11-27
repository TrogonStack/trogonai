const COOKIE = 'app_builder_session';

export interface Session {
  id: string;
  /** Set-Cookie value to send back when the request had no session yet. */
  setCookie: string | undefined;
}

/**
 * Reads the session id from the Cookie header, minting one when absent. The id names the
 * sandbox, so anyone holding it reaches that sandbox: use your own authentication in a real app.
 */
export function sessionFromRequest(request: Request): Session {
  const header = request.headers.get('cookie') ?? '';
  for (const part of header.split(';')) {
    const [name, value] = part.trim().split('=');
    if (name === COOKIE && value !== undefined && /^[0-9a-f-]{36}$/.test(value)) {
      return { id: value, setCookie: undefined };
    }
  }
  const id = crypto.randomUUID();
  return { id, setCookie: `${COOKIE}=${id}; Path=/; HttpOnly; SameSite=Lax; Max-Age=604800` };
}

export function withSessionCookie(response: Response, session: Session): Response {
  if (session.setCookie !== undefined) {
    response.headers.append('set-cookie', session.setCookie);
  }
  return response;
}

export const TOKEN_STORAGE_KEY = 'agentzero_token';
let inMemoryToken: string | null = null;

function readStorage(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // localStorage may be unavailable in some browser privacy modes
  }
}

function removeStorage(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // Ignore
  }
}

/**
 * Retrieve the stored authentication token.
 */
export function getToken(): string | null {
  if (inMemoryToken && inMemoryToken.length > 0) {
    return inMemoryToken;
  }

  const sessionToken = readStorage(TOKEN_STORAGE_KEY);
  if (sessionToken && sessionToken.length > 0) {
    inMemoryToken = sessionToken;
    return sessionToken;
  }


  return null;
}

/**
 * Store an authentication token.
 */
export function setToken(token: string): void {
  inMemoryToken = token;
  writeStorage(TOKEN_STORAGE_KEY, token);
}

/**
 * Remove the stored authentication token.
 */
export function clearToken(): void {
  inMemoryToken = null;
  removeStorage(TOKEN_STORAGE_KEY);
}

/**
 * Returns true if a token is currently stored.
 */
export function isAuthenticated(): boolean {
  const token = getToken();
  return token !== null && token.length > 0;
}

// What every journey that runs Jeden with HOME set to an isolated directory
// spreads into that program's environment.
//
// Git reads its global configuration from $HOME/.gitconfig. Under an
// isolated HOME it finds none, falls back to the system file's
// `credential.helper osxkeychain`, and a git that asks for a credential then
// has macOS store it in a login keychain that does not exist under that HOME:
// the person sees a "Keychain Not Found ... Reset To Defaults" dialog.
// GIT_CONFIG_GLOBAL names the ~/.gitconfig of the HOME the journey was
// started with and GIT_CONFIG_NOSYSTEM drops the system file, so no git a
// journey starts can reach the keychain helper.

import { homedir } from 'node:os';
import { join } from 'node:path';

export const pinnedGit = Object.freeze({
  GIT_CONFIG_GLOBAL: join(homedir(), '.gitconfig'),
  GIT_CONFIG_NOSYSTEM: '1',
});

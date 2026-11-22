// The ONE rename prompt for a shell session, shared by the `/shells` rows and the plain-shell top
// bar so both surfaces literally open the same dialog. An Ionic alert — never the browser's native
// prompt, which blocks the page and looks foreign on a phone — mirroring
// `TerminalPage.renameInitiative` and `FilesPage.promptName`.
//
// The controller is a PARAMETER typed by a local structural slice, not an import of Ionic: the
// plugin-less web vitest cannot load `@ionic/angular`, and this keeps the prompt testable with a fake
// while the real `AlertController` still satisfies the type at the call sites.
import { SESSION_TITLE_MAX } from './shells-page-logic';

/** The subset of Ionic `AlertOptions` this prompt sets. */
export interface RenameAlertOptions {
  header: string;
  inputs: {
    name: string;
    type: 'text';
    value: string;
    placeholder: string;
    attributes: { maxlength: number };
  }[];
  buttons: { text: string; role: 'cancel' | 'confirm' }[];
}

/** Structural slice of Ionic's `AlertController` — lets the spec pass a fake. */
export interface RenameAlertController {
  create(opts: RenameAlertOptions): Promise<{
    present(): Promise<void>;
    onDidDismiss(): Promise<{ role?: string; data?: any }>;
  }>;
}

/**
 * Ask for a new shell title. Resolves the RAW input string on Save (trimming, capping and the
 * unchanged check are `renameIntent`'s job — one rule in one place), or `null` on Cancel / backdrop /
 * any other dismissal. The prefill is the trimmed current title, or empty: prefilling the display
 * fallback "Shell" would save that placeholder text by accident. User text only ever goes into the
 * input's `value`, never the alert's HTML `message`.
 */
export async function promptSessionRename(
  alertCtrl: RenameAlertController,
  currentTitle: string | null | undefined,
): Promise<string | null> {
  const alert = await alertCtrl.create({
    header: 'Rename shell',
    inputs: [
      {
        name: 'title',
        type: 'text',
        value: (currentTitle ?? '').trim(),
        placeholder: 'Shell',
        attributes: { maxlength: SESSION_TITLE_MAX },
      },
    ],
    buttons: [
      { text: 'Cancel', role: 'cancel' },
      { text: 'Save', role: 'confirm' },
    ],
  });
  await alert.present();
  const { role, data } = await alert.onDidDismiss();
  if (role !== 'confirm') return null;
  return String(data?.values?.title ?? '');
}

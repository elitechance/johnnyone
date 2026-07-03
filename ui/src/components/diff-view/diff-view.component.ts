import {
  Component,
  Input,
  ChangeDetectionStrategy,
  OnChanges,
  signal,
  inject,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { DomSanitizer, SafeHtml } from '@angular/platform-browser';
import { highlightCode } from '../../lib/markdown-render';
import type { GitDiffFile, GitDiffView } from '../../services/johnny-api.service';
import {
  parseUnifiedDiff,
  langForPath,
  fileTotals,
  DiffLineKind,
} from './diff-parse';

// Re-export the pure helpers so consumers/tests can reach them via the component path or the
// `@johnnyone/ui` barrel (the pure module carries no Angular import — decision D10).
export { parseUnifiedDiff, langForPath, fileTotals } from './diff-parse';
export type { DiffLine, DiffLineKind } from './diff-parse';

/** One diff line prepared for the template: highlighted code (add/del/context) or plain text
 * (hunk/meta). `html` is already sanitized (hljs output only, same posture as `johnny-markdown-view`). */
interface RenderedLine {
  kind: DiffLineKind;
  marker: string;
  html: SafeHtml | null;
  plain: string | null;
}

/**
 * Working-tree diff renderer (overhaul P7, D10): a changed-file list with per-file +/- counts and
 * the selected file's hunks, each code line syntax-highlighted by REUSING the P3 render core
 * (`highlightCode`) — no second highlighter. Mirrors `johnny-transcript-view`'s shape. Matches mock
 * §04. A clean/non-repo `GitDiffView` (or none) shows a benign empty state (D11).
 */
@Component({
  selector: 'johnny-diff-view',
  standalone: true,
  imports: [CommonModule],
  templateUrl: './diff-view.component.html',
  styleUrls: ['./diff-view.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class DiffViewComponent implements OnChanges {
  private readonly sanitizer = inject(DomSanitizer);

  @Input() view: GitDiffView | null = null;

  /** Path of the currently selected file (defaults to the first changed file). */
  readonly selectedPath = signal<string | null>(null);

  ngOnChanges(): void {
    const files = this.files;
    const current = this.selectedPath();
    // Keep the selection if the file is still present; otherwise fall back to the first file.
    if (!current || !files.some((f) => f.path === current)) {
      this.selectedPath.set(files[0]?.path ?? null);
    }
  }

  get files(): GitDiffFile[] {
    return this.view?.files ?? [];
  }

  /** Benign empty state: a clean repo, no changed files, or no view at all. */
  get isEmpty(): boolean {
    return !this.view || this.view.clean || this.files.length === 0;
  }

  get totals(): { count: number; additions: number; deletions: number } {
    return fileTotals(this.files);
  }

  get selectedFile(): GitDiffFile | null {
    const path = this.selectedPath();
    return this.files.find((f) => f.path === path) ?? null;
  }

  /** The selected file's diff, parsed + prepared (highlighted) for the template. */
  get lines(): RenderedLine[] {
    const file = this.selectedFile;
    if (!file) return [];
    const lang = langForPath(file.path);
    return parseUnifiedDiff(file.diff).map((line) => {
      if (line.kind === 'add' || line.kind === 'del' || line.kind === 'context') {
        const marker =
          line.kind === 'add' ? '+' : line.kind === 'del' ? '-' : ' ';
        // Strip the leading marker char; highlight the remaining code text.
        const code = line.text.slice(1);
        const html = highlightCode(code, lang).html;
        return {
          kind: line.kind,
          marker,
          html: this.sanitizer.bypassSecurityTrustHtml(html),
          plain: null,
        };
      }
      // hunk / meta lines render as plain text (no highlighting).
      return { kind: line.kind, marker: '', html: null, plain: line.text };
    });
  }

  select(path: string): void {
    this.selectedPath.set(path);
  }
}

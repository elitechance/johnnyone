import { Component, Input, Output, EventEmitter, ChangeDetectionStrategy, ViewChildren, QueryList } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonList,
  IonItem,
  IonButton,
  IonIcon,
  IonLabel,
  IonItemSliding,
  IonItemOptions,
  IonItemOption,
} from '@ionic/angular/standalone';
import { AiSession } from '../../models/ai-session.model';

@Component({
  selector: 'johnny-session-list',
  standalone: true,
  imports: [
    CommonModule,
    IonList,
    IonItem,
    IonButton,
    IonIcon,
    IonLabel,
    IonItemSliding,
    IonItemOptions,
    IonItemOption,
  ],
  templateUrl: './session-list.component.html',
  styleUrls: ['./session-list.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class SessionListComponent {
  @Input() sessions: AiSession[] = [];
  @Input() activeSessionId: string | null = null;

  @Output() sessionSelected = new EventEmitter<string>();
  @Output() sessionArchived = new EventEmitter<string>();
  @Output() sessionDeleted = new EventEmitter<string>();

  @ViewChildren(IonItemSliding) slidingItems!: QueryList<IonItemSliding>;

  private closeAllSliding(): void {
    this.slidingItems?.forEach(item => item.close());
  }

  get activeSessions(): AiSession[] {
    return this.sessions.filter((s) => s.status === 'active');
  }

  get archivedSessions(): AiSession[] {
    return this.sessions.filter((s) => s.status === 'archived' || s.status === 'completed');
  }

  onSelect(session: AiSession): void {
    this.closeAllSliding();
    this.sessionSelected.emit(session.id);
  }

  onArchive(event: Event, session: AiSession): void {
    event.stopPropagation();
    this.closeAllSliding();
    this.sessionArchived.emit(session.id);
  }

  onDelete(event: Event, session: AiSession): void {
    event.stopPropagation();
    this.closeAllSliding();
    this.sessionDeleted.emit(session.id);
  }

  formatDate(dateStr: string): string {
    if (!dateStr) return '';
    const date = new Date(dateStr);
    const now = new Date();
    const diffMs = now.getTime() - date.getTime();
    const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

    if (diffDays === 0) {
      return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    } else if (diffDays === 1) {
      return 'Yesterday';
    } else if (diffDays < 7) {
      return date.toLocaleDateString([], { weekday: 'short' });
    } else {
      return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
    }
  }

  formatTokens(tokens: number): string {
    if (tokens < 1000) return String(tokens);
    if (tokens < 1000000) return `${(tokens / 1000).toFixed(1)}k`;
    return `${(tokens / 1000000).toFixed(1)}M`;
  }

  statusColor(status: AiSession['status']): string {
    switch (status) {
      case 'active':
        return 'success';
      case 'completed':
        return 'primary';
      case 'archived':
        return 'medium';
      default:
        return 'medium';
    }
  }

  trackBySessionId(_index: number, session: AiSession): string {
    return session.id;
  }
}

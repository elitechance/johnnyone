import { Component, OnInit, OnDestroy, inject } from '@angular/core';
import { CommonModule } from '@angular/common';
import {
  IonHeader,
  IonToolbar,
  IonTitle,
  IonContent,
  IonButtons,
  IonBackButton,
  IonBadge,
  IonCard,
  IonCardHeader,
  IonCardTitle,
  IonCardSubtitle,
  IonCardContent,
  IonRefresher,
  IonRefresherContent,
  IonChip,
  IonProgressBar,
} from '@ionic/angular/standalone';
import { Router } from '@angular/router';
import { TauriBridgeService, ToolExecution } from '../../services/tauri-bridge.service';
import { Subject, takeUntil, interval } from 'rxjs';

@Component({
  selector: 'app-tools',
  standalone: true,
  imports: [
    CommonModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonButtons,
    IonBackButton,
    IonBadge,
    IonCard,
    IonCardHeader,
    IonCardTitle,
    IonCardSubtitle,
    IonCardContent,
    IonRefresher,
    IonRefresherContent,
    IonChip,
    IonProgressBar,
  ],
  templateUrl: './tools.page.html',
  styleUrls: ['./tools.page.scss'],
})
export class ToolsPage implements OnInit, OnDestroy {
  private readonly tauriBridge = inject(TauriBridgeService);
  private readonly router = inject(Router);
  private readonly destroy$ = new Subject<void>();

  toolExecutions: ToolExecution[] = [];
  loading = false;

  ngOnInit(): void {
    this.loadToolExecutions();
    this.startPolling();
  }

  ngOnDestroy(): void {
    this.destroy$.next();
    this.destroy$.complete();
  }

  private startPolling(): void {
    interval(2000)
      .pipe(takeUntil(this.destroy$))
      .subscribe(() => {
        this.loadToolExecutions();
      });
  }

  async loadToolExecutions(): Promise<void> {
    try {
      this.toolExecutions = await this.tauriBridge.getToolExecutions();
    } catch (err) {
      console.error('Failed to load tool executions:', err);
    }
  }

  async handleRefresh(event: any): Promise<void> {
    await this.loadToolExecutions();
    event.target.complete();
  }

  getStatusColor(status: string): string {
    switch (status) {
      case 'pending':
        return 'warning';
      case 'running':
        return 'primary';
      case 'completed':
        return 'success';
      case 'failed':
        return 'danger';
      case 'cancelled':
        return 'medium';
      default:
        return 'medium';
    }
  }

  getToolIcon(toolName: string): string {
    switch (toolName) {
      case 'shell':
        return 'terminal-outline';
      case 'filesystem':
        return 'folder-outline';
      case 'process':
        return 'hardware-chip-outline';
      case 'browser':
        return 'globe-outline';
      default:
        return 'construct-outline';
    }
  }

  formatDuration(startedAt: string, completedAt?: string): string {
    const start = new Date(startedAt).getTime();
    const end = completedAt ? new Date(completedAt).getTime() : Date.now();
    const durationMs = end - start;

    if (durationMs < 1000) return `${durationMs}ms`;
    if (durationMs < 60000) return `${(durationMs / 1000).toFixed(1)}s`;
    return `${(durationMs / 60000).toFixed(1)}m`;
  }

  goBack(): void {
    this.router.navigate(['/chat']);
  }
}

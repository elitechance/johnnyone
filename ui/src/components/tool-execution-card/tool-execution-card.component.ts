import { Component, Input, Output, EventEmitter, ChangeDetectionStrategy } from '@angular/core';
import { CommonModule } from '@angular/common';
import { IonicModule } from '@ionic/angular';
import { ToolExecution } from '../../models/tool.model';

@Component({
  selector: 'johnny-tool-execution-card',
  standalone: true,
  imports: [CommonModule, IonicModule],
  templateUrl: './tool-execution-card.component.html',
  styleUrls: ['./tool-execution-card.component.scss'],
  changeDetection: ChangeDetectionStrategy.OnPush,
})
export class ToolExecutionCardComponent {
  @Input({ required: true }) execution!: ToolExecution;

  @Output() approved = new EventEmitter<string>();
  @Output() rejected = new EventEmitter<string>();
  @Output() cancelled = new EventEmitter<string>();

  showInput = false;
  showOutput = false;

  get isPending(): boolean {
    return this.execution.status === 'pending';
  }

  get isRunning(): boolean {
    return this.execution.status === 'running' || this.execution.status === 'approved';
  }

  get isCompleted(): boolean {
    return this.execution.status === 'completed';
  }

  get isFailed(): boolean {
    return this.execution.status === 'failed';
  }

  get statusColor(): string {
    switch (this.execution.status) {
      case 'pending':
        return 'warning';
      case 'approved':
      case 'running':
        return 'primary';
      case 'completed':
        return 'success';
      case 'failed':
        return 'danger';
      case 'rejected':
        return 'danger';
      case 'cancelled':
        return 'medium';
      default:
        return 'medium';
    }
  }

  get statusIcon(): string {
    switch (this.execution.status) {
      case 'pending':
        return 'time-outline';
      case 'approved':
      case 'running':
        return 'sync-outline';
      case 'completed':
        return 'checkmark-circle-outline';
      case 'failed':
        return 'close-circle-outline';
      case 'rejected':
        return 'ban-outline';
      case 'cancelled':
        return 'stop-circle-outline';
      default:
        return 'help-circle-outline';
    }
  }

  get formattedInput(): string {
    try {
      return JSON.stringify(this.execution.input, null, 2);
    } catch {
      return String(this.execution.input);
    }
  }

  get formattedDuration(): string {
    if (!this.execution.durationMs) return '';
    if (this.execution.durationMs < 1000) {
      return `${this.execution.durationMs}ms`;
    }
    return `${(this.execution.durationMs / 1000).toFixed(1)}s`;
  }

  toggleInput(): void {
    this.showInput = !this.showInput;
  }

  toggleOutput(): void {
    this.showOutput = !this.showOutput;
  }

  onApprove(): void {
    this.approved.emit(this.execution.id);
  }

  onReject(): void {
    this.rejected.emit(this.execution.id);
  }

  onCancel(): void {
    this.cancelled.emit(this.execution.id);
  }
}

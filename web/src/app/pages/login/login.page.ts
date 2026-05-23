import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import {
  IonButton,
  IonCard,
  IonContent,
  IonHeader,
  IonInput,
  IonItem,
  IonList,
  IonText,
  IonTitle,
  IonToolbar,
} from '@ionic/angular/standalone';
import { GRAPHQL_API_URL } from '@johnnyone/ui';
import { AuthService } from '../../services/auth.service';

@Component({
  selector: 'app-login-page',
  standalone: true,
  imports: [
    FormsModule,
    IonHeader,
    IonToolbar,
    IonTitle,
    IonContent,
    IonCard,
    IonList,
    IonItem,
    IonInput,
    IonButton,
    IonText,
  ],
  templateUrl: './login.page.html',
  styleUrl: './login.page.scss',
})
export class LoginPage {
  private readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly auth = inject(AuthService);
  private readonly route = inject(ActivatedRoute);
  private readonly router = inject(Router);

  email = signal('');
  password = signal('');
  tenantId = signal('00000000-0000-0000-0000-000000000001');
  loading = signal(false);
  error = signal('');

  async login(): Promise<void> {
    if (this.loading()) return;

    this.error.set('');
    this.loading.set(true);

    try {
      await this.auth.login(
        this.apiUrl,
        this.email().trim(),
        this.password(),
        this.tenantId().trim(),
      );
      const returnUrl =
        this.route.snapshot.queryParamMap.get('returnUrl') || '/chat';
      await this.router.navigateByUrl(returnUrl);
    } catch (err) {
      this.error.set(err instanceof Error ? err.message : String(err));
    } finally {
      this.loading.set(false);
    }
  }
}

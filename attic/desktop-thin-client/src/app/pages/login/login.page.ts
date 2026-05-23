import { CommonModule } from '@angular/common';
import { Component, inject } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { ActivatedRoute, Router } from '@angular/router';
import { GRAPHQL_API_URL } from '@johnnyone/ui';
import { AuthService } from '../../services/auth.service';

@Component({
  selector: 'app-login',
  standalone: true,
  imports: [CommonModule, FormsModule],
  templateUrl: './login.page.html',
  styleUrls: ['./login.page.scss'],
})
export class LoginPage {
  private readonly apiUrl = inject(GRAPHQL_API_URL);
  private readonly auth = inject(AuthService);
  private readonly route = inject(ActivatedRoute);
  private readonly router = inject(Router);

  email = '';
  password = '';
  tenantId = '00000000-0000-0000-0000-000000000001';
  loading = false;
  error = '';

  async login(): Promise<void> {
    if (this.loading) return;

    this.error = '';
    this.loading = true;

    try {
      await this.auth.login(this.apiUrl, this.email.trim(), this.password, this.tenantId.trim());
      const returnUrl = this.route.snapshot.queryParamMap.get('returnUrl') || '/chat';
      await this.router.navigateByUrl(returnUrl);
    } catch (err) {
      this.error = err instanceof Error ? err.message : String(err);
    } finally {
      this.loading = false;
    }
  }
}

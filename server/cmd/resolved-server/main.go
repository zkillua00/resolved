package main

import (
	"bufio"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"resolved-server/internal/bootstrap"
	"resolved-server/internal/config"
	"resolved-server/internal/identity"
	"resolved-server/internal/users"

	"golang.org/x/term"
)

func main() {
	if err := run(os.Args[1:], os.Stdin, os.Stdout, os.Stderr); err != nil {
		reportRunError(os.Stderr, err)
		os.Exit(1)
	}
}

func reportRunError(stderr io.Writer, err error) {
	_, _ = fmt.Fprintf(stderr, "resolved-server: %v\n", err)
}

func run(args []string, stdin *os.File, stdout, stderr io.Writer) error {
	if len(args) == 0 {
		return errors.New("expected a command: serve or bootstrap-admin")
	}
	cfg, err := config.Load()
	if err != nil {
		return err
	}

	switch args[0] {
	case "serve":
		if len(args) != 1 {
			return errors.New("serve does not accept arguments")
		}
		return serve(cfg, stdout)
	case "bootstrap-admin":
		return bootstrapAdmin(cfg, args[1:], stdin, stdout, stderr)
	default:
		return fmt.Errorf("unknown command %q; expected serve or bootstrap-admin", args[0])
	}
}

func serve(cfg config.Config, accessLog io.Writer) error {
	application, err := bootstrap.New(cfg, accessLog)
	if err != nil {
		return err
	}
	defer application.Close()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	listenErrors := make(chan error, 1)
	go func() {
		listenErrors <- application.Server.Start()
	}()

	log.Printf("Resolved collaboration server listening on %s with %s", cfg.Address, cfg.Database.Driver)
	select {
	case err := <-listenErrors:
		return err
	case <-ctx.Done():
		shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		if err := application.Server.Shutdown(shutdownCtx); err != nil {
			return fmt.Errorf("shut down server: %w", err)
		}
		return nil
	}
}

func bootstrapAdmin(
	cfg config.Config,
	args []string,
	stdin *os.File,
	stdout, stderr io.Writer,
) error {
	flags := flag.NewFlagSet("bootstrap-admin", flag.ContinueOnError)
	flags.SetOutput(stderr)
	email := flags.String("email", "", "login identifier for the initial owner")
	name := flags.String("name", "Owner", "display name for the initial owner")
	if err := flags.Parse(args); err != nil {
		return err
	}
	if flags.NArg() != 0 || strings.TrimSpace(*email) == "" {
		return errors.New("bootstrap-admin requires an --email login identifier and accepts no positional arguments")
	}

	password, err := readPassword(stdin, stderr)
	if err != nil {
		return err
	}
	application, err := bootstrap.New(cfg, io.Discard)
	if err != nil {
		return err
	}
	defer application.Close()

	user, err := application.Users.BootstrapOwner(context.Background(), users.CreateInput{
		Email:       *email,
		DisplayName: *name,
		Password:    password,
		RoleIDs:     []string{identity.OwnerRoleID},
	})
	if err != nil {
		return err
	}
	_, err = fmt.Fprintf(stdout, "Created deployment owner %s (%s).\n", user.DisplayName, user.Email)
	return err
}

func readPassword(stdin *os.File, stderr io.Writer) (string, error) {
	fd := int(stdin.Fd())
	if term.IsTerminal(fd) {
		if _, err := fmt.Fprint(stderr, "Password: "); err != nil {
			return "", err
		}
		first, err := term.ReadPassword(fd)
		if err != nil {
			return "", fmt.Errorf("read password: %w", err)
		}
		if _, err := fmt.Fprint(stderr, "\nConfirm password: "); err != nil {
			return "", err
		}
		second, err := term.ReadPassword(fd)
		if err != nil {
			return "", fmt.Errorf("confirm password: %w", err)
		}
		_, _ = fmt.Fprintln(stderr)
		if string(first) != string(second) {
			return "", errors.New("passwords do not match")
		}
		return string(first), nil
	}

	line, err := bufio.NewReader(stdin).ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		return "", fmt.Errorf("read password from stdin: %w", err)
	}
	return strings.TrimRight(line, "\r\n"), nil
}

package requestproxy

import (
	"golang.org/x/net/publicsuffix"
	"net/http"
	"net/http/cookiejar"
	"net/url"
	"resolved-server/internal/workspaces"
)

// A fresh jar belongs to one execution. It never lives on the shared client.
type executionCookieJar struct {
	jar      http.CookieJar
	updates  []workspaces.JarCookie
	explicit bool
}

func newExecutionCookieJar(snapshot workspaces.CookieJar, explicit bool) *executionCookieJar {
	jar, _ := cookiejar.New(&cookiejar.Options{PublicSuffixList: publicsuffix.List})
	for _, item := range snapshot.Cookies {
		origin, err := url.Parse(item.URL)
		if err != nil {
			continue
		}
		cookie, err := http.ParseSetCookie(item.Cookie)
		if err != nil {
			continue
		}
		jar.SetCookies(origin, []*http.Cookie{cookie})
	}
	return &executionCookieJar{jar: jar, explicit: explicit}
}
func (j *executionCookieJar) Cookies(u *url.URL) []*http.Cookie {
	if j.explicit {
		return nil
	}
	return j.jar.Cookies(u)
}
func (j *executionCookieJar) SetCookies(u *url.URL, cookies []*http.Cookie) {
	j.jar.SetCookies(u, cookies)
	for _, cookie := range cookies {
		j.updates = append(j.updates, workspaces.JarCookie{URL: u.String(), Cookie: cookie.String()})
	}
}
